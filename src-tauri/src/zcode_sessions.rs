//! ZCode 项目维度派生库（M5 阶段）。
//!
//! 背景：ZCode 主库 ~/.zcode/cli/db/db.sqlite 的 model_usage 表没有 cwd/
//! project 类列，项目浏览器里 zcode 用量只能整体聚合进 __unknown__。
//! 本模块解析 ZCode 本机会话文件，自建「带项目维度的镜像」派生库
//! ~/.zbar/zcode_sessions.sqlite，补齐 session_id → 项目目录 的映射。
//!
//! 口径铁律（防双计）：主面板统计/菜单栏/同步的 zcode 用量数据源永远是
//! db.sqlite 本体（db.rs，一行不改）；项目浏览器 zcode 部分以 db.sqlite 的
//! model_usage 行为唯一用量来源，仅通过 ATTACH 本派生库补项目归属——
//! 两侧读的是同一张表，总量严格一致，永不相加。
//!
//! 实测数据源（2026-08 采样，ZCode 私有格式、无 schema 承诺，逐字段容错）：
//! 1. ~/.zcode/cli/rollout/model-io-sess_<uuid>.jsonl：每行一次模型请求，
//!    顶层 sessionId（"sess_<uuid>" / 子代理 "sess_subagent_agent_<uuid>"）、
//!    startedAt（RFC3339 毫秒）、model.modelId、response.usage 内
//!    inputTokens/outputTokens/totalTokens/cacheReadTokens/cacheWriteTokens
//!    （无 reasoning 类字段）。注意：ZCode 会定期清理旧 rollout 文件，
//!    本机仅存当日活跃的 3 个文件（69 行），而主库当日已有 2000+ 行——
//!    rollout 覆盖严重不全，不能作为项目浏览器的用量来源，只做镜像留存；
//! 2. ~/.zcode/cli/agents/sess_<id>/agent_<id>/metadata.json：子代理元数据，
//!    含 cwd（缺失时用 workspaceRoot）、childSessionId（子代理会话）、
//!    parentSessionId（主会话）、createdAt/updatedAt（RFC3339）；
//! 3. db.sqlite 的 session 表：id + directory 列（完整项目路径，实测 1584/
//!    1584 全量覆盖，time_created/time_updated 为毫秒整数）。这是覆盖最全、
//!    结构最稳的 cwd 来源（外键保证 model_usage.session_id 必有 session 行），
//!    列缺失（未来版本改名）时自动跳过，退回 metadata.json 扫描。
//!
//! 导入器沿用 codex.rs 模式：file_progress 偏移增量续读 + IMPORT_LOCK 串行 +
//! 幂等 upsert（哨兵语义：找不到 cwd 也落一条 project_key 为 NULL 的行，
//! 避免重复扫描；已有值绝不覆盖，仅补 NULL）。

use rusqlite::Connection;
use serde::Deserialize;
use std::collections::HashMap;
use std::io::{BufRead, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};

use crate::pricing::config_dir;

// ===== 路径定位 =====

/// ZCode CLI 根目录（~/.zcode/cli）。
/// 设置了 ZBAR_DB（指向 <cli>/db/db.sqlite）时取其上两级，保证自定义数据
/// 目录与 db.rs 的主库定位一致；否则用默认 ~/.zcode/cli。
fn cli_root() -> PathBuf {
    if let Ok(p) = std::env::var("ZBAR_DB") {
        let pb = PathBuf::from(p.trim());
        // db.sqlite 位于 <cli>/db/db.sqlite，文件 → db 目录 → cli 根
        if let Some(root) = pb.parent().and_then(|db| db.parent()) {
            return root.to_path_buf();
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".zcode")
        .join("cli")
}

/// rollout 目录（~/.zcode/cli/rollout，不存在时返回 None）。
fn rollout_dir() -> Option<PathBuf> {
    let dir = cli_root().join("rollout");
    dir.is_dir().then_some(dir)
}

/// agents 目录（~/.zcode/cli/agents，不存在时返回 None）。
fn agents_dir() -> Option<PathBuf> {
    let dir = cli_root().join("agents");
    dir.is_dir().then_some(dir)
}

/// 派生库路径：~/.zbar/zcode_sessions.sqlite
pub(crate) fn derived_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("zcode_sessions.sqlite"))
}

/// 派生库的显式只读 ATTACH URI：file:<路径>?mode=ro。
/// 背景：ATTACH 打开被附加库的读写模式是「以主连接 openFlags 为初值、
/// 由 URI 的 mode 参数覆盖」，未指定 mode 时是否沿用主连接只读属隐式
/// 行为（随 SQLite 版本/构建变化，本机 bundled 实测会沿用，但不应依赖
/// 这种传导），只有 file: URI 显式携带 mode=ro 才是独立于主连接 flags
/// 的稳定只读保证（详见 tests::attach_mode_ro_rejects_writes_on_temp_copy）。
/// 路径中的 URI 保留字符（% ? #）先做百分号转义，防用户目录含特殊字符
/// 时解析错位。
pub(crate) fn derived_db_attach_uri() -> Result<String, String> {
    let raw = derived_db_path()?.to_string_lossy().to_string();
    let esc = raw
        .replace('%', "%25")
        .replace('?', "%3f")
        .replace('#', "%23");
    Ok(format!("file:{esc}?mode=ro"))
}

/// 打开主库只读连接（与 db::open_db 同口径，额外启用 SQLITE_OPEN_URI）。
/// URI flag 是 ATTACH 'file:...?mode=ro' 显式生效的前提：只有主连接开启
/// URI 时（或 SQLite 编译期定义了 SQLITE_USE_URI），ATTACH 的文件名才被
/// 按 URI 解析，否则整串会被当成普通路径文件名。db::open_db 未开启该
/// flag，为不改动 db.rs 的公共查询入口，查询侧（projects.rs / 测试）
/// 需要显式只读 ATTACH 时单独用此函数打开。
pub(crate) fn open_main_db_readonly_uri() -> Result<Connection, String> {
    let path = crate::db::db_path()?;
    let conn = Connection::open_with_flags(
        &path,
        rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
            | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
            | rusqlite::OpenFlags::SQLITE_OPEN_URI,
    )
    .map_err(|e| format!("打开数据库失败: {e}"))?;
    // 与 db::open_db 同口径：只读连接在 WAL 下读取也需等待写锁释放
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// 打开（必要时创建）派生库并确保表结构就绪。
/// 刻意不设 WAL：查询侧会 ATTACH 本库；WAL 库的读取依赖 -shm/-wal 辅助
/// 文件（最后写连接关闭时被删除），对读取方的目录权限有额外要求。本项目
/// 写入频率极低（30 秒节流的增量导入），默认 journal 模式足够，且显式
/// mode=ro 只读 ATTACH（见 derived_db_attach_uri）无需任何 WAL 辅助文件
/// 即可读取。
fn open_derived_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let conn = Connection::open(derived_db_path()?)
        .map_err(|e| format!("打开 ZCode 会话派生库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(10))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS session_meta (
            session_id  TEXT PRIMARY KEY,
            cwd         TEXT,
            project_key TEXT,
            first_at    INTEGER,
            last_at     INTEGER
        );
        CREATE TABLE IF NOT EXISTS model_usage (
            session_id           TEXT,
            seq                  INTEGER,
            started_at           INTEGER,
            model_id             TEXT,
            input_tokens         INTEGER,
            output_tokens        INTEGER,
            cache_read_tokens    INTEGER,
            cache_write_tokens   INTEGER,
            reasoning_tokens     INTEGER,
            computed_total_tokens INTEGER,
            PRIMARY KEY (session_id, seq)
        );
        CREATE TABLE IF NOT EXISTS file_progress (
            path   TEXT PRIMARY KEY,
            offset INTEGER,
            mtime  INTEGER
        );
        CREATE INDEX IF NOT EXISTS idx_zs_mu_session ON model_usage(session_id);
        CREATE INDEX IF NOT EXISTS idx_zs_mu_time ON model_usage(started_at);",
    )
    .map_err(|e| format!("初始化 ZCode 会话派生库失败: {e}"))?;
    Ok(conn)
}

// ===== JSON 解析结构（全部 Option/默认值容错，坏行只跳过不中断）=====

/// rollout JSONL 单行（字段名 2026-08 实测）：
/// 顶层 sessionId/startedAt(RFC3339)/model.modelId/response.usage。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutLine {
    #[serde(default)]
    session_id: Option<String>,
    #[serde(default)]
    started_at: Option<String>,
    #[serde(default)]
    model: Option<RolloutModel>,
    #[serde(default)]
    response: Option<RolloutResponse>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutModel {
    #[serde(default)]
    model_id: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutResponse {
    #[serde(default)]
    usage: Option<RolloutUsage>,
}

/// 单次请求的 token 用量（实测字段：inputTokens/outputTokens/totalTokens/
/// cacheReadTokens/cacheWriteTokens；无 reasoning 类字段，恒 0）。
/// 实测 totalTokens = input + output（不含 cache）。
#[derive(Debug, Default, Deserialize)]
#[serde(rename_all = "camelCase")]
struct RolloutUsage {
    #[serde(default)]
    input_tokens: i64,
    #[serde(default)]
    output_tokens: i64,
    #[serde(default)]
    total_tokens: i64,
    #[serde(default)]
    cache_read_tokens: i64,
    #[serde(default)]
    cache_write_tokens: i64,
}

/// agents/**/metadata.json（字段名 2026-08 实测）：cwd 优先、workspaceRoot
/// 兜底；childSessionId 是子代理自己的会话 id，parentSessionId 是派生它的
/// 主会话 id，两者 cwd 相同，各写一条映射。
#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
struct AgentMetadata {
    #[serde(default)]
    cwd: Option<String>,
    #[serde(default)]
    workspace_root: Option<String>,
    #[serde(default)]
    child_session_id: Option<String>,
    #[serde(default)]
    parent_session_id: Option<String>,
    #[serde(default)]
    created_at: Option<String>,
    #[serde(default)]
    updated_at: Option<String>,
}

impl AgentMetadata {
    /// 非空 cwd（cwd 缺失/为空时回退 workspaceRoot）。
    fn effective_cwd(&self) -> Option<&str> {
        self.cwd
            .as_deref()
            .map(str::trim)
            .filter(|c| !c.is_empty())
            .or_else(|| {
                self.workspace_root
                    .as_deref()
                    .map(str::trim)
                    .filter(|c| !c.is_empty())
            })
    }
}

/// RFC3339 时间戳（如 2026-08-28T14:15:47.212Z）→ 毫秒时间戳。
/// 主库 started_at 是毫秒整数，rollout/metadata 的时间字段是 ISO 字符串，
/// 这里统一转毫秒对齐主库口径。
fn parse_ts_ms(ts: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(ts)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

// ===== 增量导入 =====

/// 导入互斥锁：项目浏览器查询/同步上传可能并发触发导入，串行化避免同一
/// 文件被双份解析（幂等 upsert 可去重，但重复 IO 浪费）。
static IMPORT_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn import_lock() -> &'static Mutex<()> {
    IMPORT_LOCK.get_or_init(|| Mutex::new(()))
}

/// 上次导入时间（30 秒节流：项目浏览器 30s 一轮查询，会话文件是分钟级
/// 追加，30 秒足够实时且省掉重复的目录递归扫描）。
static LAST_IMPORT_AT: OnceLock<Mutex<Option<std::time::Instant>>> = OnceLock::new();

fn last_import_at() -> &'static Mutex<Option<std::time::Instant>> {
    LAST_IMPORT_AT.get_or_init(|| Mutex::new(None))
}

/// 增量导入（查询入口用，30 秒节流；失败也计入节流窗口，避免故障时
/// 重试风暴）。rollout/agents 目录不存在（未安装 ZCode / 旧版本无此目录）
/// 时静默返回 Ok，不报错。
pub fn import_incremental() -> Result<(), String> {
    {
        let mut last = last_import_at()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if last.map(|t| t.elapsed() < std::time::Duration::from_secs(30)) == Some(true) {
            return Ok(());
        }
        *last = Some(std::time::Instant::now());
    }
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conn = open_derived_db()?;
    import_rollout_files(&mut conn);
    import_session_meta_from_main_db(&mut conn);
    import_session_meta_from_agents(&mut conn);
    Ok(())
}

/// 预载全部文件进度（path → (offset, mtime)）。
fn load_file_progress(conn: &Connection) -> Result<HashMap<String, (u64, i64)>, String> {
    let mut stmt = conn
        .prepare("SELECT path, offset, mtime FROM file_progress")
        .map_err(|e| format!("读取 ZCode 会话导入进度失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("读取 ZCode 会话导入进度失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 ZCode 会话导入进度失败: {e}"))?;
    Ok(rows
        .into_iter()
        .map(|(p, off, mt)| (p, (off.max(0) as u64, mt)))
        .collect())
}

/// 收集 rollout 目录下的 model-io-*.jsonl（不存在/不可读返回空，不报错）。
fn collect_rollout_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with("model-io-") && name.ends_with(".jsonl") {
            out.push(entry.path());
        }
    }
}

/// 阶段一：增量导入 rollout 用量镜像。
/// 单文件失败只记日志跳过，不阻断其他文件与后续阶段。
fn import_rollout_files(conn: &mut Connection) {
    let Some(dir) = rollout_dir() else {
        return;
    };
    let progress = match load_file_progress(conn) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 读取导入进度失败（下次重试）: {e}");
            return;
        }
    };
    let mut files = Vec::new();
    collect_rollout_files(&dir, &mut files);
    files.sort();
    for path in &files {
        let key = path.to_string_lossy().to_string();
        if let Err(e) = import_one_rollout_file(conn, path, progress.get(&key).copied()) {
            eprintln!("[zbar-zcode-sessions] 导入 {} 失败（下次重试）: {e}", path.display());
        }
    }
}

/// 解析单个 rollout 文件的增量部分（file_progress 偏移续读）。
/// - seq 取物理行号（文件内递增，含坏行占号）：追加型文件行号天然稳定，
///   重解析时同号靠主键冲突 IGNORE 幂等；
/// - 文件变短（被重写）→ 从头重解析；
/// - 时间字段实测为 startedAt（RFC3339）；无 startedAt 的行无法归入统计
///   区间，跳过并计数（实测每行都有，不做文件 mtime 兜底）；
/// - 解析失败/缺字段的行计数，文件级汇总限流打印，避免刷屏。
fn import_one_rollout_file(
    conn: &mut Connection,
    path: &Path,
    known: Option<(u64, i64)>,
) -> Result<(), String> {
    let meta = std::fs::metadata(path).map_err(|e| format!("读取文件元信息失败: {e}"))?;
    let size = meta.len();
    let mtime = meta
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0);

    // 增量判据（对齐 codex 导入器，纯 offset/size；追加写入必更新 mtime，
    // 不能用 mtime 判断是否有新增）：
    // - 已读偏移超出当前文件长度（变短/重写）→ 从头重解析（唯一键幂等）；
    // - 偏移已到文件末尾 → 无新增，直接返回；
    // - 其余 → 从记录偏移续读。
    let start_offset = match known {
        Some((off, _)) if off > size => 0,
        Some((off, _)) if off >= size => return Ok(()),
        Some((off, _)) => off,
        None => 0,
    };

    let mut file = std::fs::File::open(path).map_err(|e| format!("打开会话文件失败: {e}"))?;
    let mut reader = std::io::BufReader::new(&mut file);
    if start_offset > 0 {
        reader
            .seek(SeekFrom::Start(start_offset))
            .map_err(|e| format!("定位读取偏移失败: {e}"))?;
    }

    let tx = conn
        .transaction()
        .map_err(|e| format!("开启导入事务失败: {e}"))?;

    let mut pos = start_offset;
    let mut last_complete_end = start_offset;
    let mut seq: i64 = 0;
    let mut bad_lines = 0usize;
    let mut buf: Vec<u8> = Vec::with_capacity(8192);
    loop {
        buf.clear();
        let n = reader
            .read_until(b'\n', &mut buf)
            .map_err(|e| format!("读取会话文件失败: {e}"))?;
        if n == 0 {
            break;
        }
        pos += n as u64;
        seq += 1;
        if buf.last() == Some(&b'\n') {
            last_complete_end = pos;
        }

        // 逐行容错：JSON 解析失败/关键字段缺失只跳过该行（偏移仍推进）
        let Ok(line) = serde_json::from_slice::<RolloutLine>(&buf) else {
            bad_lines += 1;
            continue;
        };
        let (Some(session_id), Some(started_at_raw)) = (&line.session_id, &line.started_at)
        else {
            bad_lines += 1;
            continue;
        };
        let Some(usage) = line.response.as_ref().and_then(|r| r.usage.as_ref()) else {
            bad_lines += 1;
            continue;
        };
        let Some(started_at) = parse_ts_ms(started_at_raw) else {
            bad_lines += 1;
            continue;
        };
        let model_id = line
            .model
            .as_ref()
            .and_then(|m| m.model_id.as_deref())
            .unwrap_or("");
        // 实测 totalTokens 恒有值且 = input + output；缺失时按同口径兜底
        let computed = if usage.total_tokens > 0 {
            usage.total_tokens
        } else {
            usage.input_tokens + usage.output_tokens
        };
        tx.execute(
            "INSERT OR IGNORE INTO model_usage
                (session_id, seq, started_at, model_id,
                 input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
                 reasoning_tokens, computed_total_tokens)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 0, ?9)",
            rusqlite::params![
                session_id,
                seq,
                started_at,
                model_id,
                usage.input_tokens,
                usage.output_tokens,
                usage.cache_read_tokens,
                usage.cache_write_tokens,
                computed,
            ],
        )
        .map_err(|e| format!("写入 ZCode 会话用量记录失败: {e}"))?;
    }

    // 进度对齐到最后一条完整行末尾（末尾半行下次追加完整后重读，幂等）
    let key = path.to_string_lossy().to_string();
    tx.execute(
        "INSERT INTO file_progress (path, offset, mtime) VALUES (?1, ?2, ?3)
         ON CONFLICT(path) DO UPDATE SET offset = ?2, mtime = ?3",
        rusqlite::params![key, last_complete_end as i64, mtime],
    )
    .map_err(|e| format!("记录 ZCode 会话导入进度失败: {e}"))?;

    tx.commit()
        .map_err(|e| format!("提交 ZCode 会话导入事务失败: {e}"))?;
    if bad_lines > 0 {
        eprintln!(
            "[zbar-zcode-sessions] {} 中有 {bad_lines} 行解析失败已跳过（私有格式字段缺失，不影响其余行）",
            path.display()
        );
    }
    Ok(())
}

/// 写入/补齐会话的项目维度记录（幂等，哨兵语义对齐 codex.session_meta）：
/// - 首次：直接插入（cwd 缺失时 project_key 存 NULL，作哨兵防止重复扫描）；
/// - 冲突：仅当库内 project_key 仍为 NULL 时允许补值/补时间（自愈早期无
///   cwd 的哨兵行），已有 project_key 的行绝不覆盖（各来源 cwd 一致，
///   先到者优先，避免来回改写）。
fn upsert_session_meta(
    conn: &Connection,
    session_id: &str,
    cwd_raw: Option<&str>,
    first_at: Option<i64>,
    last_at: Option<i64>,
) -> Result<(), String> {
    let project_key = cwd_raw.and_then(crate::projects::normalize_cwd);
    conn.execute(
        "INSERT INTO session_meta (session_id, cwd, project_key, first_at, last_at)
         VALUES (?1, ?2, ?3, ?4, ?5)
         ON CONFLICT(session_id) DO UPDATE SET
            cwd = COALESCE(excluded.cwd, session_meta.cwd),
            project_key = COALESCE(excluded.project_key, session_meta.project_key),
            first_at = CASE
                WHEN session_meta.first_at IS NULL THEN excluded.first_at
                WHEN excluded.first_at IS NULL THEN session_meta.first_at
                WHEN excluded.first_at < session_meta.first_at THEN excluded.first_at
                ELSE session_meta.first_at END,
            last_at = CASE
                WHEN session_meta.last_at IS NULL THEN excluded.last_at
                WHEN excluded.last_at IS NULL THEN session_meta.last_at
                WHEN excluded.last_at > session_meta.last_at THEN excluded.last_at
                ELSE session_meta.last_at END
         WHERE session_meta.project_key IS NULL",
        rusqlite::params![session_id, cwd_raw, project_key, first_at, last_at],
    )
    .map_err(|e| format!("写入 ZCode 会话项目维度失败: {e}"))?;
    Ok(())
}

/// 派生库 session_meta 全量快照：session_id → 是否已有 project_key。
fn existing_session_meta(conn: &Connection) -> Result<HashMap<String, bool>, String> {
    let mut stmt = conn
        .prepare("SELECT session_id, project_key IS NOT NULL FROM session_meta")
        .map_err(|e| format!("查询 ZCode 会话项目维度失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, bool>(1)?)))
        .map_err(|e| format!("查询 ZCode 会话项目维度失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("查询 ZCode 会话项目维度失败: {e}"))?;
    Ok(rows.into_iter().collect())
}

/// 阶段二：从主库 session 表回填项目维度（cwd 覆盖最全的来源）。
/// 只读打开主库（绝不写入）；session 表无 id/directory 列（未来版本改名）
/// 时静默跳过，退回 metadata.json 来源。每轮全量拉主库 session 行在 Rust
/// 侧过滤「派生库缺失/无键」的候选，稳定后只剩两次 SELECT、无写事务。
fn import_session_meta_from_main_db(conn: &mut Connection) {
    let Ok(main) = crate::db::open_db() else {
        return; // 主库不可用（未安装 ZCode）：本阶段静默跳过
    };
    // 列探测：id/directory 必须存在；时间列缺失时置 NULL
    let has = |col: &str| crate::db::has_column(&main, "session", col);
    if !has("id") || !has("directory") {
        return;
    }
    let (tc, tu) = (has("time_created"), has("time_updated"));
    let id_col = "id";
    let sql = format!(
        "SELECT \"{id_col}\", directory{tc_sql}{tu_sql} FROM session",
        tc_sql = if tc { ", time_created" } else { "" },
        tu_sql = if tu { ", time_updated" } else { "" },
    );
    let Ok(mut stmt) = main.prepare(&sql) else {
        return;
    };
    let rows = match stmt.query_map([], |row| {
        Ok((
            row.get::<_, String>(0)?,
            row.get::<_, Option<String>>(1)?,
            if tc { row.get::<_, Option<i64>>(2)? } else { None },
            if tu { row.get::<_, Option<i64>>(3)? } else { None },
        ))
    }) {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 读取主库 session 表失败（下次重试）: {e}");
            return;
        }
    };
    let rows = match rows.collect::<Result<Vec<_>, _>>() {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 读取主库 session 行失败（下次重试）: {e}");
            return;
        }
    };

    let existing = match existing_session_meta(conn) {
        Ok(map) => map,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 查询派生库会话维度失败（下次重试）: {e}");
            return;
        }
    };
    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 开启主库回填事务失败（下次重试）: {e}");
            return;
        }
    };
    let mut written = 0usize;
    for (session_id, directory, created, updated) in rows {
        let has_key = existing.get(&session_id).copied().unwrap_or(false);
        let cwd = directory.as_deref().map(str::trim).filter(|c| !c.is_empty());
        if has_key {
            continue; // 已有归属，绝不覆盖
        }
        // 库内已有哨兵行且新值也无 cwd：跳过，避免每轮重复 upsert
        if existing.contains_key(&session_id) && cwd.is_none() {
            continue;
        }
        if let Err(e) = upsert_session_meta(&tx, &session_id, cwd, created, updated) {
            eprintln!("[zbar-zcode-sessions] 回填 {session_id} 项目维度失败（下次重试）: {e}");
            continue;
        }
        written += 1;
    }
    if let Err(e) = tx.commit() {
        eprintln!("[zbar-zcode-sessions] 提交主库回填事务失败（下次重试）: {e}");
        return;
    }
    if written > 0 {
        eprintln!("[zbar-zcode-sessions] 从主库 session 表回填了 {written} 条会话项目维度");
    }
}

/// 递归收集 agents 目录下的 metadata.json（sess_<id>/agent_<id>/ 三层，
/// 上限 4 层防御异常嵌套；目录不存在返回空）。
fn collect_agent_metadata(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if depth > 0 {
                collect_agent_metadata(&path, depth - 1, out);
            }
        } else if entry.file_name().to_string_lossy() == "metadata.json" {
            out.push(path);
        }
    }
}

/// 阶段三：扫描 agents/**/metadata.json 补充项目维度（主库 session 表
/// 不可用/缺列时的兜底来源；cwd 与主库实测一致）。
/// file_progress 记 mtime 做增量：mtime 未变跳过，变了全量重读（upsert 幂等）。
/// 每个文件提供两条映射：childSessionId（子代理会话）与 parentSessionId
/// （主会话），cwd 相同。
fn import_session_meta_from_agents(conn: &mut Connection) {
    let Some(dir) = agents_dir() else {
        return;
    };
    let progress = match load_file_progress(conn) {
        Ok(p) => p,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 读取导入进度失败（下次重试）: {e}");
            return;
        }
    };
    let mut files = Vec::new();
    collect_agent_metadata(&dir, 4, &mut files);
    files.sort();

    let tx = match conn.transaction() {
        Ok(tx) => tx,
        Err(e) => {
            eprintln!("[zbar-zcode-sessions] 开启 metadata 回填事务失败（下次重试）: {e}");
            return;
        }
    };
    let mut scanned = 0usize;
    let mut bad_files = 0usize;
    for path in &files {
        let key = path.to_string_lossy().to_string();
        let Ok(meta) = std::fs::metadata(path) else {
            bad_files += 1;
            continue;
        };
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_millis() as i64)
            .unwrap_or(0);
        let size = meta.len() as i64;
        // mtime 未变且已记录过（offset 存该文件大小）→ 跳过
        if let Some((_, prev_mtime)) = progress.get(&key) {
            if *prev_mtime == mtime {
                continue;
            }
        }
        scanned += 1;
        let Ok(text) = std::fs::read_to_string(path) else {
            bad_files += 1;
            continue;
        };
        let Ok(data) = serde_json::from_str::<AgentMetadata>(&text) else {
            bad_files += 1;
            continue;
        };
        let cwd = data.effective_cwd();
        let first_at = data.created_at.as_deref().and_then(parse_ts_ms);
        let last_at = data.updated_at.as_deref().and_then(parse_ts_ms);
        for session_id in [&data.child_session_id, &data.parent_session_id]
            .into_iter()
            .flatten()
        {
            let trimmed = session_id.trim();
            if trimmed.is_empty() {
                continue;
            }
            if let Err(e) = upsert_session_meta(&tx, trimmed, cwd, first_at, last_at) {
                eprintln!("[zbar-zcode-sessions] 回填 {trimmed} 项目维度失败（下次重试）: {e}");
            }
        }
        let _ = tx.execute(
            "INSERT INTO file_progress (path, offset, mtime) VALUES (?1, ?2, ?3)
             ON CONFLICT(path) DO UPDATE SET offset = ?2, mtime = ?3",
            rusqlite::params![key, size, mtime],
        );
    }
    if let Err(e) = tx.commit() {
        eprintln!("[zbar-zcode-sessions] 提交 metadata 回填事务失败（下次重试）: {e}");
        return;
    }
    if bad_files > 0 {
        eprintln!(
            "[zbar-zcode-sessions] agents 目录有 {bad_files} 个 metadata.json 读取/解析失败已跳过"
        );
    }
    let _ = scanned;
}

// ===== 查询（projects.rs / sync.rs 用）=====

/// 派生库是否已有可用项目映射（存在 project_key 非 NULL 的行）。
/// 库不可用/无映射返回 false，调用方回退旧路径（全量进 __unknown__），
/// 保证不丢总量。
pub fn has_project_mapping() -> bool {
    let conn = match open_derived_db() {
        Ok(conn) => conn,
        Err(_) => return false,
    };
    conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM session_meta WHERE project_key IS NOT NULL LIMIT 1)",
        [],
        |row| row.get::<_, bool>(0),
    )
    .unwrap_or(false)
}

/// 全量会话映射：session_id → (project_key, 原始 cwd)。
/// 仅返回有 project_key 的行；供同步上传填充会话/项目维度（查不到的
/// 调用方保持 None）。库不可用返回空（调用方降级）。
pub fn session_project_map() -> HashMap<String, (String, String)> {
    let conn = match open_derived_db() {
        Ok(conn) => conn,
        Err(_) => return HashMap::new(),
    };
    let Ok(mut stmt) = conn.prepare(
        "SELECT session_id, project_key, cwd FROM session_meta
         WHERE project_key IS NOT NULL AND cwd IS NOT NULL AND cwd != ''",
    ) else {
        return HashMap::new();
    };
    let Ok(rows) = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                (row.get::<_, String>(1)?, row.get::<_, String>(2)?),
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    else {
        return HashMap::new();
    };
    rows.into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// rollout 行解析：实测字段（sessionId/startedAt/model.modelId/
    /// response.usage 的 camelCase token 字段）→ 派生库口径；无 reasoning
    /// 字段恒 0；totalTokens 缺失时按 input+output 兜底。
    #[test]
    fn rollout_line_field_mapping() {
        let line: RolloutLine = serde_json::from_str(
            r#"{"sessionId":"sess_subagent_agent_84644f6c",
                "startedAt":"2026-08-28T15:11:38.986Z",
                "model":{"modelId":"GLM-5.3-Flash","role":"subagent"},
                "response":{"usage":{"inputTokens":13827,"outputTokens":344,
                    "totalTokens":14171,"cacheReadTokens":4800,"cacheWriteTokens":0}}}"#,
        )
        .expect("rollout 行解析失败");
        assert_eq!(line.session_id.as_deref(), Some("sess_subagent_agent_84644f6c"));
        assert_eq!(parse_ts_ms(line.started_at.as_deref().unwrap()), Some(1_787_929_898_986));
        let usage = line.response.unwrap().usage.unwrap();
        assert_eq!(usage.input_tokens, 13827);
        assert_eq!(usage.output_tokens, 344);
        assert_eq!(usage.cache_read_tokens, 4800);
        assert_eq!(usage.cache_write_tokens, 0);
        assert_eq!(line.model.unwrap().model_id.as_deref(), Some("GLM-5.3-Flash"));

        // 关键字段缺失的行：结构可解析但 sessionId/usage 为 None（导入时跳过）
        let partial: RolloutLine =
            serde_json::from_str(r#"{"type":"model_io","completedAt":"2026-08-28T14:16:07.259Z"}"#)
                .expect("部分字段行解析失败");
        assert!(partial.session_id.is_none());
        assert!(partial.started_at.is_none());
        assert!(partial.response.is_none());
        assert!(partial.model.is_none());

        // totalTokens 缺失 → input + output 兜底（导入逻辑同款口径）
        let fallback: RolloutUsage = serde_json::from_str(
            r#"{"inputTokens":100,"outputTokens":7}"#,
        )
        .expect("兜底 usage 解析失败");
        assert_eq!(fallback.input_tokens + fallback.output_tokens, 107);
    }

    /// metadata.json 解析：cwd 优先、workspaceRoot 兜底；child/parent 两个
    /// 会话 id 都要提取（实测字段名 camelCase）。
    #[test]
    fn agent_metadata_cwd_fallback() {
        let full: AgentMetadata = serde_json::from_str(
            r#"{"agentId":"agent_x","childSessionId":"sess_subagent_agent_x",
                "parentSessionId":"sess_root","cwd":"/Users/a/proj ",
                "workspaceRoot":"/Users/a/proj","createdAt":"2026-07-10T01:16:38.534Z",
                "updatedAt":"2026-07-10T01:18:18.143Z"}"#,
        )
        .expect("metadata 解析失败");
        assert_eq!(full.effective_cwd(), Some("/Users/a/proj"));
        assert_eq!(full.child_session_id.as_deref(), Some("sess_subagent_agent_x"));
        assert_eq!(full.parent_session_id.as_deref(), Some("sess_root"));
        assert_eq!(parse_ts_ms(full.created_at.as_deref().unwrap()).unwrap(), 1_783_646_198_534);

        // cwd 缺失/空白 → workspaceRoot；两者都缺 → None
        let no_cwd: AgentMetadata = serde_json::from_str(
            r#"{"childSessionId":"c","parentSessionId":"p","cwd":"","workspaceRoot":"/w/r"}"#,
        )
        .expect("无 cwd metadata 解析失败");
        assert_eq!(no_cwd.effective_cwd(), Some("/w/r"));
        let none: AgentMetadata =
            serde_json::from_str(r#"{"childSessionId":"c"}"#).expect("空 metadata 解析失败");
        assert_eq!(none.effective_cwd(), None);
    }

    /// 目录收集的容错：不存在的目录返回空、不 panic（rollout 与 agents 两个
    /// 来源共用此口径，旧版本 ZCode 无这些目录时优雅降级）。
    #[test]
    fn collect_files_on_missing_dir_returns_empty() {
        let mut out = Vec::new();
        collect_rollout_files(Path::new("/nonexistent/zbar/rollout"), &mut out);
        assert!(out.is_empty());
        collect_agent_metadata(Path::new("/nonexistent/zbar/agents"), 4, &mut out);
        assert!(out.is_empty());
    }

    /// 显式只读 ATTACH 语义验证（/tmp 临时副本，不触碰真实库）：
    /// 1) 生产路径（只读主连接 + URI flag，同 open_main_db_readonly_uri）：
    ///    ATTACH 'file:...?mode=ro' 后读取正常、写语句被拒（attempt to
    ///    write a readonly database）——派生库「绝不写入」由 mode=ro
    ///    强制，而非「没有写语句」的约定；
    /// 2) 机制隔离（读写主连接）：同一文件 mode=ro 写仍被拒、DETACH 后
    ///    mode=rw 可写——证明读写模式由 URI 的 mode 参数决定；读写主
    ///    连接下不带 mode 的 ATTACH 会按读写打开，这正是必须显式
    ///    mode=ro 的原因（本项目主连接虽只读，但只读保证不应依赖主
    ///    连接 flags 的隐式传导与版本行为）。
    #[test]
    fn attach_mode_ro_rejects_writes_on_temp_copy() {
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or_default();
        let dir = std::env::temp_dir().join(format!("zbar_ro_attach_test_{}_{}", std::process::id(), nanos));
        std::fs::create_dir_all(&dir).expect("创建临时目录失败");
        // 目标库（模拟派生库本体是普通可写库）
        let copy = dir.join("copy.sqlite");
        {
            let w = Connection::open(&copy).expect("创建临时目标库失败");
            w.execute_batch(
                "CREATE TABLE t1 (id INTEGER PRIMARY KEY, v TEXT);
                 INSERT INTO t1 (v) VALUES ('x');",
            )
            .expect("初始化临时目标库失败");
        }
        // 只读主连接 + URI flag（与 open_main_db_readonly_uri 同口径）
        let main = dir.join("main.sqlite");
        {
            let m = Connection::open(&main).expect("创建临时主库失败");
            m.execute_batch("CREATE TABLE dummy (x)").expect("初始化临时主库失败");
        }
        let conn = Connection::open_with_flags(
            &main,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY
                | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX
                | rusqlite::OpenFlags::SQLITE_OPEN_URI,
        )
        .expect("打开临时主库失败");
        let esc = copy
            .to_string_lossy()
            .to_string()
            .replace('%', "%25")
            .replace('?', "%3f")
            .replace('#', "%23");

        // mode=ro ATTACH：读取正常
        conn.execute(
            "ATTACH DATABASE ?1 AS zs_ro",
            rusqlite::params![format!("file:{esc}?mode=ro")],
        )
        .expect("ATTACH mode=ro 应成功");
        let n: i64 = conn
            .query_row("SELECT COUNT(*) FROM zs_ro.t1", [], |r| r.get(0))
            .expect("mode=ro ATTACH 后应可读");
        assert_eq!(n, 1);
        // 核心断言：mode=ro ATTACH 后写语句必须被拒
        let err = conn
            .execute("INSERT INTO zs_ro.t1 (v) VALUES ('y')", [])
            .expect_err("mode=ro ATTACH 后写语句必须被拒绝");
        let msg = err.to_string();
        assert!(
            msg.contains("readonly") || msg.contains("read-only"),
            "期望只读库错误，实际: {msg}"
        );

        // 机制隔离：读写主连接（默认 READ_WRITE|CREATE flags）下同一文件
        let rwconn = Connection::open(&main).expect("读写打开临时主库失败");
        // mode=ro → 写仍被拒（拒绝来自 mode 参数，而非主连接只读）
        rwconn
            .execute(
                "ATTACH DATABASE ?1 AS zs_ro2",
                rusqlite::params![format!("file:{esc}?mode=ro")],
            )
            .expect("读写主连接 ATTACH mode=ro 应成功");
        let err = rwconn
            .execute("INSERT INTO zs_ro2.t1 (v) VALUES ('y2')", [])
            .expect_err("读写主连接下 mode=ro ATTACH 写语句仍必须被拒绝");
        let msg = err.to_string();
        assert!(
            msg.contains("readonly") || msg.contains("read-only"),
            "期望只读库错误，实际: {msg}"
        );
        rwconn
            .execute("DETACH DATABASE zs_ro2", [])
            .expect("DETACH 失败");
        // mode=rw → 可写（同一连接、同一文件，仅 mode 参数不同）
        rwconn
            .execute(
                "ATTACH DATABASE ?1 AS zs_rw",
                rusqlite::params![format!("file:{esc}?mode=rw")],
            )
            .expect("ATTACH mode=rw 应成功");
        rwconn
            .execute("INSERT INTO zs_rw.t1 (v) VALUES ('z')", [])
            .expect("mode=rw 的 ATTACH 应可写");

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// 派生库 upsert 哨兵语义：首次落库（含 NULL 哨兵）、后到补值、
    /// 已有值绝不覆盖。使用真实派生库（~/.zbar，与 codex 测试同款口径，
    /// 测试会话 id 加 test_ 前缀避免污染真实数据）。
    #[test]
    fn upsert_session_meta_sentinel_semantics() {
        let mut conn = open_derived_db().expect("打开派生库失败");
        let tx = conn.transaction().expect("开启事务失败");
        // 首次：无 cwd → NULL 哨兵
        upsert_session_meta(&tx, "test_sess_sentinel", None, Some(100), Some(200))
            .expect("哨兵写入失败");
        // 后到：补上 cwd
        upsert_session_meta(&tx, "test_sess_sentinel", Some("/Users/a/proj"), Some(50), Some(150))
            .expect("补值失败");
        // 再到：已有值不覆盖
        upsert_session_meta(&tx, "test_sess_sentinel", Some("/other/path"), Some(1), Some(999))
            .expect("幂等重放失败");
        tx.commit().expect("提交失败");

        let (cwd, key, first, last): (String, String, i64, i64) = conn
            .query_row(
                "SELECT cwd, project_key, first_at, last_at FROM session_meta
                 WHERE session_id = 'test_sess_sentinel'",
                [],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?)),
            )
            .expect("查询哨兵行失败");
        assert_eq!(cwd, "/Users/a/proj");
        // 归一化键（大小写折叠按平台断言）
        let expect_key = crate::projects::normalize_cwd("/Users/a/proj").unwrap();
        assert_eq!(key, expect_key);
        // 时间：首次插入 (100,200)；补值轮 (50,150) → first 取更早 50，
        // last 保留更大的 200；已有值后的第三轮不更新
        assert_eq!((first, last), (50, 200));

        conn.execute("DELETE FROM session_meta WHERE session_id = 'test_sess_sentinel'", [])
            .expect("清理测试数据失败");
    }

    /// 冒烟：新路径（主库 model_usage + 派生库映射 ATTACH）与主库直查的
    /// 今日总量必须严格一致——两侧读同一张主表，差异恒为 0（rollout 镜像
    /// 不参与聚合，避免其覆盖不全造成缩水）。依赖本机 ZCode 当日有用量，
    /// 无数据时跳过断言。
    #[test]
    fn derived_project_totals_match_main_db_today() {
        use chrono::Timelike;
        let now = chrono::Local::now();
        let from_ms = now.timestamp_millis()
            - (now.time().num_seconds_from_midnight() as i64) * 1000
            - now.timestamp_subsec_millis() as i64;
        let to_ms = now.timestamp_millis() + 60_000;

        // 先触发一次导入（30 秒节流可能命中缓存，不影响断言：映射缺漏只
        // 影响项目归属分布，不影响总量一致性）
        let _ = import_incremental();

        // 主库只读连接带 SQLITE_OPEN_URI（ATTACH 的 file:...?mode=ro URI
        // 只有主连接启用 URI 时才被解析），与 projects.rs 查询路径同机制
        let conn = open_main_db_readonly_uri().expect("主库打开失败（本机无 ZCode？）");
        let derived = derived_db_path().expect("派生库路径失败");
        if !derived.exists() {
            eprintln!("冒烟跳过：派生库尚未生成");
            return;
        }
        // 显式只读 ATTACH：读写模式必须由 URI mode 参数显式指定，
        // 未指定 mode 时的隐式行为随版本变化，不可依赖
        let derived_uri = derived_db_attach_uri().expect("派生库 URI 构造失败");
        conn.execute("ATTACH DATABASE ?1 AS zs", rusqlite::params![derived_uri])
            .expect("ATTACH 派生库失败");

        let (main_tokens, main_reqs): (i64, i64) = conn
            .query_row(
                "SELECT COALESCE(SUM(computed_total_tokens),0), COUNT(*)
                 FROM model_usage WHERE started_at >= ?1 AND started_at < ?2",
                rusqlite::params![from_ms, to_ms],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("主库直查失败");
        if main_reqs == 0 {
            eprintln!("冒烟跳过：主库今日无用量");
            return;
        }

        let (proj_tokens, proj_reqs, unknown_tokens): (i64, i64, i64) = conn
            .query_row(
                "SELECT COALESCE(SUM(mu.computed_total_tokens),0), COUNT(*),
                        COALESCE(SUM(CASE WHEN sm.project_key IS NULL
                                          THEN mu.computed_total_tokens ELSE 0 END),0)
                 FROM model_usage mu
                 LEFT JOIN zs.session_meta sm ON sm.session_id = mu.session_id
                 WHERE mu.started_at >= ?1 AND mu.started_at < ?2",
                rusqlite::params![from_ms, to_ms],
                |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
            )
            .expect("ATTACH 聚合查询失败");

        assert_eq!(proj_tokens, main_tokens, "新路径与主库直查的今日 tokens 必须一致");
        assert_eq!(proj_reqs, main_reqs, "新路径与主库直查的今日请求数必须一致");
        let mapped_pct = if main_tokens > 0 {
            (main_tokens - unknown_tokens) as f64 / main_tokens as f64 * 100.0
        } else {
            0.0
        };
        eprintln!(
            "冒烟结果: 今日新路径 tokens={proj_tokens} requests={proj_reqs} 与主库一致（差异 0%）；\
             已归属真实项目 {mapped_pct:.1}%（unknown tokens={unknown_tokens}）"
        );
        assert!(main_tokens > 0);
    }
}

//! Kimi Code 用量统计模块。
//!
//! 数据来源：Kimi Code CLI 把每个会话记录在
//! ~/.kimi-code/sessions/<sessionId>/agents/<agentId>/wire.jsonl
//! （append-only，每行一个 JSON 事件；首行为会话 metadata，其余为 llm.request /
//! usage.record 等事件，本模块只关心最后一种）。token 用量在 type=usage.record
//! 行：usage.{inputOther, output, inputCacheRead, inputCacheCreation} 四项之和
//! 即总量，time 为毫秒时间戳，模型名直接取行内 model 字段。
//!
//! 与 Claude 模块的关键差异：
//! - **session_id 提取**：所有会话文件都叫 wire.jsonl，无法像 Claude 那样用
//!   文件名区分会话，改用路径回溯——仅当结构严格符合 sessions/<sessionId>/
//!   agents/<agentId>/wire.jsonl 时取 sessionId 目录名；嵌套/非标准布局退回
//!   文件全路径做全局唯一兜底。
//! - **去重键**：usage.record 行无唯一 id（不同于 Claude 的 message.id），用
//!   "<session_id>|<文件相对路径>|<usage 序号>"三段，序号只对 usage.record 行
//!   递增且按文件隔离（同一 session 下多个 agent 文件不得共享序号序列）；
//!   续读从该文件已入库最大序号恢复，文件被重写（变短）时从 0 重计，
//!   UNIQUE 键 + "总量更大者胜" upsert 保证幂等。
//! - **耗时数据**：无流式首 token 事件，TTFT 恒 None（同 Claude 只有总耗时
//!   的口径）。总耗时用事件时间差推算：同文件内顺序处理时维护"最近一条
//!   llm.request 的 time"，usage.record 到来时取差值作 duration_ms——连续
//!   多条 llm.request（重试）天然取最近一条即成功那次；差值 <100ms 或
//!   >600s（10 分钟）或无前置请求视为脏配对写 NULL，不兜底。
//! - **额度来源**：会话文件里没有 rate_limits，订阅额度实时调
//!   GET {apiBase}/usages（Kimi Code CLI 内部同款）。域名按 CLI 的
//!   ~/.kimi-code/region 分流：global → api.kimi.ai/coding/v1，否则（含
//!   文件缺失）默认 api.kimi.com/coding/v1。凭据优先 ~/.zbar/kimi.json 的
//!   api_key（用户显式配置，长期有效），否则读 ~/.kimi-code/credentials/
//!   *.json：OAuth 结构（access_token + expires_at + refresh_token 三者
//!   齐全）的 access_token 有效期仅 15 分钟且只有 CLI 运行时才刷新，应用
//!   打开时大概率已过期，故过期时用 refresh_token 调 POST
//!   {oauthHost}/api/oauth/token 换新——新 token 只存内存缓存，绝不写回
//!   凭据文件（CLI 自己管理该文件，应用只读；refresh_token 实测非
//!   rotation 型可复用，内存刷新不影响 CLI）。
//!
//! 实现方式与 claude.rs 同构：原始 jsonl 只读 + 派生自有库 ~/.zbar/kimi.sqlite，
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

/// Kimi sessions 目录路径（不做存在性检查，供诊断展示）。
/// 环境变量 ZBAR_KIMI_HOME（指向 .kimi-code 根目录）优先，否则 ~/.kimi-code/sessions。
fn sessions_dir_path() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_KIMI_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home).join("sessions");
        }
    }
    let home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
    home.join(".kimi-code").join("sessions")
}

/// 定位 Kimi 会话目录。目录不存在返回友好中文错误（调用方按需容错降级）。
pub fn sessions_dir() -> Result<PathBuf, String> {
    let p = sessions_dir_path();
    if p.is_dir() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 Kimi Code 会话目录: {}。请确认 Kimi Code CLI 已安装并使用过，或设置 ZBAR_KIMI_HOME 环境变量指向 .kimi-code 根目录。",
            p.display()
        ))
    }
}

/// 自有导入库路径：~/.zbar/kimi.sqlite
fn kimi_db_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("kimi.sqlite"))
}

/// 打开（必要时创建）导入库并确保表结构就绪。这是自有库，读写均可用。
/// 与 claude.sqlite 同构，含 duration_ms 列（值来自 llm.request→usage.record
/// 事件时间差，口径见模块头）；无 TTFT 列（无数据源）。旧库缺列时 ALTER
/// 补加（见 ensure_duration_column），不重建、不丢已导入数据。
///
/// 去重键 dedupe_key（全局唯一）："<session_id>|<文件相对路径>|<usage 序号>"。
/// usage.record 行无唯一 id，序号只对 usage.record 行递增，且必须按文件隔离
/// （同一 session 的 agents/<agentId>/ 多个 wire.jsonl 共享 sessionId）。文件被
/// 重写（变短）时序号从 0 重计，重放行与该文件旧行撞相同键，靠"总量更大者
/// 胜"的 upsert 幂等去重。被覆盖的行 updated_at 记录修订时间，为将来同步
/// 补传预留（与 claude.rs 语义一致）。
fn open_kimi_db() -> Result<Connection, String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    open_kimi_db_at(&kimi_db_path()?)
}

/// 进程内串行化补列迁移（claude.rs SCHEMA_MIGRATION_LOCK 同款思路）：
/// 升级后首次启动多个查询命令并发 open，避免同时判缺列 + 交错重复 ALTER
/// （第二个 ALTER 会因列已存在而报错）。
static ENSURE_DURATION_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

/// 在指定路径打开（必要时创建）导入库。表结构与生产库完全一致，
/// 供测试注入临时库文件（不触碰真实 ~/.zbar/kimi.sqlite）。
fn open_kimi_db_at(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open(path).map_err(|e| format!("打开 Kimi 导入库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;

    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         CREATE TABLE IF NOT EXISTS model_usage (
            id INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id TEXT NOT NULL,
            dedupe_key TEXT NOT NULL,
            started_at INTEGER NOT NULL,
            model_id TEXT NOT NULL DEFAULT '',
            provider_id TEXT NOT NULL DEFAULT 'kimi',
            input_tokens INTEGER NOT NULL DEFAULT 0,
            output_tokens INTEGER NOT NULL DEFAULT 0,
            cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
            cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
            reasoning_tokens INTEGER NOT NULL DEFAULT 0,
            computed_total_tokens INTEGER NOT NULL DEFAULT 0,
            duration_ms INTEGER,
            updated_at INTEGER NOT NULL DEFAULT 0,
            UNIQUE(dedupe_key)
         );
         CREATE INDEX IF NOT EXISTS idx_kimi_model_usage_started ON model_usage(started_at);
         CREATE INDEX IF NOT EXISTS idx_kimi_model_usage_updated ON model_usage(updated_at);
         CREATE TABLE IF NOT EXISTS file_progress (
            path   TEXT    PRIMARY KEY,
            offset INTEGER NOT NULL,
            size   INTEGER NOT NULL
         );",
    )
    .map_err(|e| format!("初始化 Kimi 导入库失败: {e}"))?;
    ensure_duration_column(&conn)?;
    Ok(conn)
}

/// 补列迁移：升级前创建的旧库 model_usage 无 duration_ms 列时
/// ALTER TABLE ADD COLUMN 补加（允许 NULL，旧行自然为 NULL）。加列保留
/// 既有数据与 file_progress 偏移，无需重导；新库已由 CREATE TABLE 直接
/// 带列，不走 ALTER 分支。
fn ensure_duration_column(conn: &Connection) -> Result<(), String> {
    let lock = ENSURE_DURATION_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = lock.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if db::has_column(conn, "model_usage", "duration_ms") {
        return Ok(());
    }
    conn.execute_batch("ALTER TABLE model_usage ADD COLUMN duration_ms INTEGER")
        .map_err(|e| format!("迁移 Kimi 导入库（补 duration_ms 列）失败: {e}"))
}

// ===== jsonl 行解析结构（未知字段自动忽略，巨型请求行不会物化）=====

/// wire.jsonl 单行事件。只取关心的类型/模型/用量/毫秒时间戳。
#[derive(Debug, Deserialize)]
struct WireLine {
    #[serde(rename = "type", default)]
    line_type: Option<String>,
    #[serde(default)]
    model: Option<String>,
    #[serde(default)]
    usage: Option<UsagePayload>,
    /// 毫秒时间戳（usage.record 行自带，直接使用，无需时区换算）
    #[serde(default)]
    time: Option<i64>,
}

/// 单次 API 调用的 token 用量。字段映射到 zcode 口径：
/// inputOther→input、output→output、inputCacheRead→cache_read、
/// inputCacheCreation→cache_creation；无独立 reasoning 字段（恒 0），
/// 无 total 字段（四项之和即总量）。
#[derive(Debug, Default, Deserialize)]
struct UsagePayload {
    #[serde(rename = "inputOther", default)]
    input_other: i64,
    #[serde(default)]
    output: i64,
    #[serde(rename = "inputCacheRead", default)]
    input_cache_read: i64,
    #[serde(rename = "inputCacheCreation", default)]
    input_cache_creation: i64,
}

impl UsagePayload {
    fn computed_total(&self) -> i64 {
        self.input_other + self.output + self.input_cache_read + self.input_cache_creation
    }
}

/// 从 wire.jsonl 路径提取会话标识。
/// 标准层级为 sessions/<sessionId>/agents/<agentId>/wire.jsonl，但文件名恒为
/// wire.jsonl（与 Claude 的 <uuid>.jsonl 不同），无法用 file_stem 区分会话，
/// 改用路径回溯。仅当结构严格符合标准层级（agents 目录的父级恰为 sessions
/// 根）时才取 sessionId 目录名；嵌套/非标准布局（如 sessions/<sid>/sub/
/// agents/<ag>/wire.jsonl 会回溯到中间目录名，不同会话的同名中间目录会共享
/// session_id）一律退回文件全路径做全局唯一兜底。
fn session_id_from_path(path: &Path, sessions_root: &Path) -> String {
    let agents_dir = path
        .parent() // <agentId>
        .and_then(|p| p.parent()); // agents
    let session_dir = agents_dir.and_then(|p| p.parent()); // <sessionId>
    let is_standard = agents_dir
        .and_then(|p| p.file_name())
        .map(|n| n.to_string_lossy() == "agents")
        .unwrap_or(false)
        // sessionId 目录的父级必须恰为 sessions 根（第 4 层校验）
        && session_dir
            .and_then(|p| p.parent())
            .map(|g| g == sessions_root)
            .unwrap_or(false);
    if is_standard {
        if let Some(name) = session_dir.and_then(|p| p.file_name()) {
            let sid = name.to_string_lossy().to_string();
            if !sid.is_empty() {
                return sid;
            }
        }
    }
    path.to_string_lossy().to_string()
}

// ===== 增量导入 =====

/// 导入互斥锁：面板查询 / 托盘标题刷新可能并发触发导入，串行化避免同一
/// 文件被双份解析（唯一键可去重，但重复 IO 浪费）。
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

/// 递归收集 sessions 目录下所有 wire.jsonl。
/// 层级为 <sessionId>/agents/<agentId>/wire.jsonl，防御性下钻 6 层。
/// 结果排序保证导入顺序稳定。
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
            if name.to_string_lossy() == "wire.jsonl" {
                out.push(path);
            }
        }
    }
}

/// 增量导入（不节流）：遍历 sessions 目录，把每个 wire.jsonl 新增部分解析入库。
/// - file_progress 记录"已处理到的字节偏移"（对齐完整行末尾）；文件变短
///   （被重写）时从头重新解析，UNIQUE 键保证幂等。
/// - 每个文件一个事务：中途崩溃整体回滚，下次从旧偏移重来。
/// - 单文件失败只记日志跳过，不阻断其他文件。
pub fn import_incremental_force() -> Result<(), String> {
    let dir = sessions_dir()?;
    let db_path = kimi_db_path()?;
    import_incremental_into(&dir, &db_path)
}

/// 导入内核：指定 sessions 目录与库文件（生产入口的参数化版本，
/// 供测试注入临时目录与临时 sqlite，不依赖真实 ~/.kimi-code 与 ~/.zbar）。
fn import_incremental_into(dir: &Path, db_path: &Path) -> Result<(), String> {
    let _guard = import_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut conn = open_kimi_db_at(db_path)?;

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
    collect_session_files(&dir, 6, &mut files);
    files.sort();

    for path in &files {
        let key = path.to_string_lossy().to_string();
        let known = progress.get(&key).copied();
        // 文件维度标识：相对 sessions 根的规范化路径（strip 失败退回全路径）。
        // 同一 session 的 agents/<agentId>/ 下多个 wire.jsonl 共享 sessionId，
        // 去重键的序号序列必须按文件隔离，防止第二个 agent 文件首导（known
        // 为 None、序号从 0 重计）与第一个文件撞键被"总量更大者胜"静默吞掉
        let file_key = path
            .strip_prefix(dir)
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| path.to_string_lossy().to_string());
        if let Err(e) = import_one_file(&mut conn, dir, path, &file_key, known) {
            eprintln!(
                "[zbar-kimi] 导入 {} 失败（下次重试）: {e}",
                path.display()
            );
        }
    }
    Ok(())
}

/// 解析单个 wire.jsonl 的增量部分。known = 上次记录的 (offset, size)；
/// file_key = 相对 sessions 根的文件路径（去重键的文件维度，见调用方注释）。
fn import_one_file(
    conn: &mut Connection,
    sessions_root: &Path,
    path: &Path,
    file_key: &str,
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

    let session_id = session_id_from_path(path, sessions_root);

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
    // usage.record 无唯一 id，去重键用"会话 + 文件 + usage 序号"三段：
    // - 文件段：同一 session 的 agents/<agentId>/ 多个 wire.jsonl 共享
    //   sessionId，序号若不带文件标识，第二个文件首次导入（known=None，
    //   序号从 0 重计）必然与第一个文件撞键，"总量更大者胜"会把小值行
    //   静默吞掉丢数据，故序号序列必须按文件隔离；
    // - 续读（文件增长）：从该文件已用的最大序号继续，避免 seq:N 与该文件
    //   历史行撞键错乱合并（撞键后"总量更大者胜"会把两条不同请求折叠成一条）；
    // - 重解析（文件变短被重写）：从 0 重计，重放行与该文件旧行撞相同键，
    //   靠"总量更大者胜"幂等去重。
    let mut line_seq: i64 = 0;
    if !reparse {
        let prefix = format!("{session_id}|{file_key}|");
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
    // 修订时间：同一键的新值覆盖旧值时打上修订标记（为将来同步补传预留）
    let now_ms = chrono::Local::now().timestamp_millis();

    // 耗时配对状态：同文件顺序处理时维护"最近一条 llm.request 的 time"，
    // usage.record 到来时取差值作 duration_ms。作用域是单次导入循环——
    // 续读窗口内若无 llm.request（请求在上次已读部分）配不上就 NULL，
    // 不跨窗口猜测；连续多条 llm.request（重试）天然取最近一条。
    let mut last_request_time: Option<i64> = None;

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

        // 逐行容错：JSON 解析失败/字段异常只跳过该行（偏移仍推进）。
        // metadata 等非 usage.record 行全部忽略（protocol_version 不硬校验，
        // 未知行类型靠这里天然跳过，CLI 升级自然兼容）；llm.request 仅提取
        // time 供后续 usage.record 做耗时配对，本身不入库
        let Ok(line) = serde_json::from_slice::<WireLine>(&buf) else {
            continue;
        };
        if line.line_type.as_deref() == Some("llm.request") {
            if let Some(t) = line.time {
                last_request_time = Some(t);
            }
            continue;
        }
        if line.line_type.as_deref() != Some("usage.record") {
            continue;
        }
        // 无模型名无法归因到模型，跳过（偏移仍推进）
        let model = match line.model.as_deref() {
            Some(m) if !m.is_empty() => m.to_string(),
            _ => continue,
        };
        let Some(usage) = line.usage.as_ref() else {
            continue;
        };
        let total = usage.computed_total();
        if total <= 0 {
            // 0 值占位行 / 空调用，跳过
            continue;
        }
        // 没有时间戳无法归入统计区间，跳过（偏移仍推进）
        let Some(started_at) = line.time else {
            continue;
        };
        // 总耗时 = usage.record.time - 最近一条 llm.request.time。合理性
        // 过滤：无前置请求、差值 <100ms（同秒级脏配对）或 >600s（跨天脏
        // 配对）→ NULL（不参与速度聚合，不用其他值兜底顶替）
        let duration_ms = match last_request_time {
            Some(req_at) => {
                let diff = started_at - req_at;
                if (100..=600_000).contains(&diff) {
                    Some(diff)
                } else {
                    None
                }
            }
            None => None,
        };

        line_seq += 1;
        let dedupe_key = format!("{session_id}|{file_key}|{line_seq}");

        // 序号键在文件重写重放时会撞到旧行：仅当新行总量更大时覆盖，
        // 防御重放值意外回退为小值的脏数据。覆盖不改变自增 id（rowid 稳定），
        // 但打上 updated_at 修订标记；duration_ms 直写新值（新行胜出：
        // 重算后配不上就直写 NULL 抹掉旧值，避免新 tokens 混旧 duration
        // 得出错误的 TPS 样本；良性重放时重算结果相同无影响）。
        tx.execute(
            "INSERT INTO model_usage
                (session_id, dedupe_key, started_at, model_id, provider_id,
                 input_tokens, output_tokens, cache_read_input_tokens,
                 cache_creation_input_tokens, reasoning_tokens, computed_total_tokens,
                 duration_ms, updated_at)
             VALUES (?1, ?2, ?3, ?4, 'kimi', ?5, ?6, ?7, ?8, 0, ?9, ?10, ?11)
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
                usage.input_other,
                usage.output,
                usage.input_cache_read,
                usage.input_cache_creation,
                total,
                duration_ms,
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

// ===== 查询函数（与 claude.rs 同名同构，查 kimi.sqlite；查询前先增量导入）=====

/// 查询 [from_ms, to_ms) 内的统计（口径与 db::query_stats 完全一致）。
/// 速度口径：duration_ms 为 llm.request→usage.record 事件时间差（合理性
/// 过滤后配不上的行为 NULL，不参与速度均值）；无流式首 token 事件，
/// TTFT 恒 None（同 Claude 只有总耗时的口径）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<db::Stats, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
    // 防御式按列有无降级（迁移后恒有列，与 claude.rs 同款写法）
    let speed =
        db::speed_agg_columns(db::has_column(&conn, "model_usage", "duration_ms"), false);

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
        .map_err(|e| format!("查询 Kimi 整体统计失败: {e}"))?;

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
        .map_err(|e| format!("准备 Kimi 模型分组查询失败: {e}"))?;

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
        .map_err(|e| format!("读取 Kimi 模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Kimi 模型分组失败: {e}"))?;

    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询 Kimi 时间范围失败: {e}"))?;

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
    let conn = open_kimi_db()?;
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
        .map_err(|e| format!("准备 Kimi 趋势查询失败: {e}"))?;

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
            .map_err(|e| format!("读取 Kimi 趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 Kimi 趋势统计失败: {e}"))?;

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

/// 查询 id > since 的明细记录（同步上传预留，首期 sync.rs 不接入 kimi）。
/// source 固定 "kimi"，local_rowid = id。
#[allow(dead_code)]
pub fn query_since(since: i64, limit: usize) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
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
        .map_err(|e| format!("准备 Kimi 增量查询失败: {e}"))?;
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
                source: "kimi".into(),
            })
        })
        .map_err(|e| format!("读取 Kimi 增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Kimi 增量记录失败: {e}"))?;
    Ok(rows)
}

/// 导入库当前最大 rowid（供「待上传条数」显示用，同步预留）。
#[allow(dead_code)]
pub fn max_rowid() -> Result<i64, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(id), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询 Kimi 最大 rowid 失败: {e}"))?;
    Ok(max)
}

/// 查询修订行（updated_at > since_ts 的记录，同步补传预留，首期不接入）。
/// 文件重写重放时新值覆盖旧行（id 不变）——常规的 id > 游标查询永远选不出它，
/// 本查询按修订时间选出这些行，供服务端以"总量更大者胜"的 upsert 修正。
/// after_id 分页：按 id 升序、只取 id > after_id 的行，调用方逐批推进。
#[allow(dead_code)]
pub fn query_revised_since(
    since_ts: i64,
    after_id: i64,
    limit: usize,
) -> Result<Vec<db::UsageRow>, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
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
        .map_err(|e| format!("准备 Kimi 修订查询失败: {e}"))?;
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
                source: "kimi".into(),
            })
        })
        .map_err(|e| format!("读取 Kimi 修订记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Kimi 修订记录失败: {e}"))?;
    Ok(rows)
}

/// 列出导入库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
/// provider_id 恒为 "kimi"。
pub fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备 Kimi 模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(db::ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取 Kimi 模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 Kimi 模型列表失败: {e}"))?;
    Ok(models)
}

/// 按指定周期聚合 Kimi Token。
/// 对比页需要真实的 [reset_at, end_at) 边界，不能用只带 HH:00 的趋势 label 反推跨日周期。
pub fn query_period_buckets(
    periods: &[(i64, i64)],
) -> Result<Vec<db::PeriodBucket>, String> {
    import_incremental()?;
    let conn = open_kimi_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备 Kimi 周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询 Kimi 周期聚合失败: {e}"))?;
        out.push(db::PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

// ===== 实时额度（api.kimi.com coding usages 端点）=====

/// Kimi 订阅额度（字段口径与 CodexRateLimits/ClaudeRateLimits 对齐，前端同款渲染，
/// 额外多出加油包余额两个字段）。
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct KimiRateLimits {
    /// 会员档位名。优先取 GET {api_base}/me 的 user_level_name（官方会员档位
    /// 名，如 Moderato/Adagio/Allegro，面向用户展示）；该接口不可用时回退
    /// usages 响应的 user.membership.level（内部枚举值如 "LEVEL_BASIC"，仅作兜底）
    pub plan_type: Option<String>,
    /// 5 小时窗口已用百分比
    pub primary_pct: Option<f64>,
    /// 5 小时窗口重置时间（毫秒时间戳）
    pub primary_reset_at: Option<i64>,
    /// 周窗口已用百分比（(limit-remaining)/limit*100）
    pub secondary_pct: Option<f64>,
    /// 周窗口重置时间（毫秒时间戳）
    pub secondary_reset_at: Option<i64>,
    /// 加油包剩余额度（boosterWallet.balance.amountLeft）
    pub booster_balance: Option<f64>,
    /// 加油包本月已用（boosterWallet.monthlyUsed）
    pub booster_monthly_used: Option<f64>,
    /// 会员月总额度已用百分比（totalQuota；服务端当前普遍返回空对象 {}，
    /// 字段预埋，等填充后自动展示）
    pub monthly_pct: Option<f64>,
    /// 会员月总额度重置时间（毫秒时间戳；同样来自 totalQuota，预埋）
    pub monthly_reset_at: Option<i64>,
}

/// Kimi 配置（~/.zbar/kimi.json）。api_key 为用户显式配置的 API Key，
/// 额度请求凭据的最高优先来源（Kimi Code CLI 的 OAuth token 可能过期/无权限）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KimiConfig {
    #[serde(default)]
    pub api_key: String,
}

impl Default for KimiConfig {
    fn default() -> Self {
        Self {
            api_key: String::new(),
        }
    }
}

fn kimi_config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("kimi.json"))
}

/// 读取 Kimi 配置；文件不存在返回默认空配置（不报错）。
/// 全字段 serde 默认值，兼容旧文件缺字段（参照 cursor.rs 的 CursorConfig 模式）。
pub fn load_kimi_config() -> Result<KimiConfig, String> {
    let path = kimi_config_path()?;
    if !path.exists() {
        return Ok(KimiConfig::default());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| format!("读取 Kimi 配置失败: {e}"))?;
    serde_json::from_str::<KimiConfig>(&data).map_err(|e| format!("解析 Kimi 配置失败: {e}"))
}

/// 保存 Kimi 配置。写路径单一（设置页保存），无后台读-改-写竞争，不加锁。
pub fn save_kimi_config(cfg: &KimiConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = kimi_config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化 Kimi 配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入 Kimi 配置失败: {e}"))
}

/// Kimi Code CLI 根目录（.kimi-code，与 sessions 的环境变量口径对齐）：
/// $ZBAR_KIMI_HOME（指向 .kimi-code 根）优先，其次 $KIMI_CODE_HOME
/// （CLI 自身变量），最后 ~/.kimi-code。region 文件与 credentials 目录
/// 都挂在这个根下。
fn kimi_code_root() -> PathBuf {
    for key in ["ZBAR_KIMI_HOME", "KIMI_CODE_HOME"] {
        if let Ok(home) = std::env::var(key) {
            let home = home.trim();
            if !home.is_empty() {
                return PathBuf::from(home);
            }
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".kimi-code")
}

/// Kimi Code CLI 的 credentials 目录。
fn credentials_dir() -> PathBuf {
    kimi_code_root().join("credentials")
}

/// Kimi API 域名组（按 CLI 的 region 文件分流，两个端点共用）。
#[derive(Debug, Clone, Copy)]
struct KimiEndpoints {
    /// OAuth 刷新端点主机（POST {oauth_host}/api/oauth/token）
    oauth_host: &'static str,
    /// coding API 基址（GET {api_base}/usages）
    api_base: &'static str,
}

/// region 文件内容 → 域名组（纯函数，便于单测）：trim 后为 "global" →
/// .ai 域名；其余（含空/未知值）默认大陆 .com。
fn endpoints_for_region(region: &str) -> KimiEndpoints {
    if region.trim() == "global" {
        KimiEndpoints {
            oauth_host: "https://auth.kimi.ai",
            api_base: "https://api.kimi.ai/coding/v1",
        }
    } else {
        KimiEndpoints {
            oauth_host: "https://auth.kimi.com",
            api_base: "https://api.kimi.com/coding/v1",
        }
    }
}

/// 读 ~/.kimi-code/region 决定域名；文件缺失/读取失败按大陆默认 .com。
fn kimi_endpoints() -> KimiEndpoints {
    let region = std::fs::read_to_string(kimi_code_root().join("region")).unwrap_or_default();
    endpoints_for_region(&region)
}

/// 从凭据 JSON 中多键名容错探测 token：
/// 依次找 apiKey / api_key / accessToken / access_token / token 的非空字符串值
/// （Kimi Code CLI 不同版本/登录方式的字段名不统一，OAuth 文件带 accessToken，
/// API Key 模式带 apiKey）。
fn extract_token_from_value(v: &serde_json::Value) -> Option<String> {
    for key in ["apiKey", "api_key", "accessToken", "access_token", "token"] {
        if let Some(token) = v.get(key).and_then(|t| t.as_str()) {
            let token = token.trim();
            if !token.is_empty() {
                return Some(token.to_string());
            }
        }
    }
    None
}

/// CLI credentials 文件的 OAuth 凭据（kimi-code.json 标准结构）：
/// access_token + expires_at + refresh_token 三者齐全才视为 OAuth 模式，
/// 缺失任一则降级为普通 token 探测（extract_token_from_value）。
#[derive(Debug, Clone)]
struct KimiOAuthCredential {
    access_token: String,
    refresh_token: String,
    /// 统一折算为毫秒的过期时间
    expires_at_ms: i64,
}

/// expires_at 单位判别：> 10^12 视为毫秒原样保留，否则按秒 ×1000
/// （OAuth 凭据文件的 expires_at 专用，与 /usages 的 RFC3339 resetTime 无关）。
fn normalize_expires_at_ms(t: i64) -> i64 {
    if t > 1_000_000_000_000 {
        t
    } else {
        t.saturating_mul(1000)
    }
}

/// 从凭据 JSON 识别 OAuth 结构：access_token / refresh_token 需为非空
/// 字符串（trim 后），expires_at 需为整数；任一缺失/类型不符返回 None
/// （由调用方降级到多键名探测）。
fn parse_oauth_credential(v: &serde_json::Value) -> Option<KimiOAuthCredential> {
    let field = |key: &str| {
        v.get(key)
            .and_then(|t| t.as_str())
            .map(str::trim)
            .filter(|t| !t.is_empty())
            .map(|t| t.to_string())
    };
    Some(KimiOAuthCredential {
        access_token: field("access_token")?,
        refresh_token: field("refresh_token")?,
        expires_at_ms: normalize_expires_at_ms(v.get("expires_at").and_then(|t| t.as_i64())?),
    })
}

/// CLI credentials 文件识别出的凭据。
#[derive(Debug)]
enum KimiCredential {
    /// OAuth 结构（过期可用 refresh_token 续期）
    OAuth(KimiOAuthCredential),
    /// 纯 token 结构（apiKey/token 等单键，无过期信息，视为长期有效）
    Plain(String),
}

/// 扫描 CLI credentials 目录（按 mtime 降序：多个凭据文件并存时优先取
/// 最新修改的，旧文件可能已被替换；mtime 读取失败的排在最后）识别凭据：
/// 每个文件先按 OAuth 结构识别（可续期），失败再降级多键名容错探测
/// （Kimi Code CLI 不同版本/登录方式的字段名不统一）。
fn scan_cli_credentials() -> Result<KimiCredential, String> {
    let dir = credentials_dir();
    if !dir.is_dir() {
        return Err(
            "未找到 Kimi 凭据：请在设置中配置 Kimi API Key（保存于 ~/.zbar/kimi.json），或先登录 Kimi Code CLI"
                .into(),
        );
    }
    let mut files: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(&dir)
        .map(|entries| {
            entries
                .flatten()
                .filter_map(|e| {
                    let path = e.path();
                    if !path.extension().map(|x| x == "json").unwrap_or(false) {
                        return None;
                    }
                    let mtime = e
                        .metadata()
                        .and_then(|m| m.modified())
                        .unwrap_or(std::time::UNIX_EPOCH);
                    Some((mtime, path))
                })
                .collect()
        })
        .unwrap_or_default();
    files.sort_by(|a, b| b.0.cmp(&a.0));
    for (_, path) in files {
        let Ok(data) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(value) = serde_json::from_str::<serde_json::Value>(&data) else {
            continue;
        };
        if let Some(oauth) = parse_oauth_credential(&value) {
            return Ok(KimiCredential::OAuth(oauth));
        }
        if let Some(token) = extract_token_from_value(&value) {
            return Ok(KimiCredential::Plain(token));
        }
    }
    Err(format!(
        "未在 {} 中找到可用的 Kimi 凭据（apiKey/access_token 均缺失或为空）。请在设置中配置 Kimi API Key",
        dir.display()
    ))
}

/// 内存 OAuth token 缓存：刷新换来的新 access_token 只存内存，绝不写回
/// ~/.kimi-code/credentials/**（该文件由 CLI 自己管理，应用只读）。
#[derive(Debug, Clone)]
struct OAuthTokenCache {
    access_token: String,
    expires_at_ms: i64,
}

static OAUTH_TOKEN_CACHE: OnceLock<Mutex<Option<OAuthTokenCache>>> = OnceLock::new();

fn oauth_token_cache() -> &'static Mutex<Option<OAuthTokenCache>> {
    OAUTH_TOKEN_CACHE.get_or_init(|| Mutex::new(None))
}

/// 更新内存缓存（短暂持锁，不与任何网络请求重叠）。
fn store_oauth_cache(access_token: String, expires_at_ms: i64) {
    *oauth_token_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(OAuthTokenCache {
        access_token,
        expires_at_ms,
    });
}

/// 清空内存缓存（usages 接口对 OAuth token 返回 401/403 时调用）：内存
/// token 可能已失效（如 region 切换后旧域名签发），置 None 使下次取 token
/// 走"重读凭据文件/刷新"路径，避免按过期时间判断仍可用而持续 401。
fn clear_oauth_cache() {
    *oauth_token_cache()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = None;
}

/// token 是否可用：距过期留 60 秒提前量（access_token 有效期仅 15 分钟，
/// 防止请求发出途中刚好过期）。
fn token_usable(expires_at_ms: i64, now_ms: i64) -> bool {
    now_ms + 60_000 < expires_at_ms
}

/// OAuth 刷新端点的 client_id（Kimi Code CLI 内置值，从 CLI 源码提取实测可用）。
const KIMI_OAUTH_CLIENT_ID: &str = "17e5f671-d194-4dfb-9706-5516cb48c098";

/// POST {oauth_host}/api/oauth/token 的响应结构。响应里的 refresh_token
/// 实测为非 rotation 型（旧值可重复使用），且应用绝不写回凭据文件，
/// 故不解析该字段。
#[derive(Debug, Deserialize)]
struct OAuthTokenResponse {
    #[serde(default)]
    access_token: Option<String>,
    #[serde(default)]
    expires_in: Option<i64>,
}

/// 构建额度/刷新请求共用的 ureq Agent（10s 超时；复用 Codex 模块的代理
/// 探测——环境变量 > 系统代理 > 直连，Kimi API 在部分网络需代理才可达）。
fn build_kimi_agent() -> ureq::Agent {
    let mut builder =
        ureq::AgentBuilder::new().timeout(std::time::Duration::from_secs(10));
    if let Some(url) = crate::codex::resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[zbar-kimi] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    builder.build()
}

/// 用 refresh_token 换新 access_token（ureq 自带 form 编码，代理复用额度
/// 请求同一套）。返回的缓存条目由调用方决定只进内存，本函数不碰任何文件。
fn refresh_oauth_token(refresh_token: &str) -> Result<OAuthTokenCache, String> {
    let endpoints = kimi_endpoints();
    let resp = build_kimi_agent()
        .post(&format!("{}/api/oauth/token", endpoints.oauth_host))
        .send_form(&[
            ("client_id", KIMI_OAUTH_CLIENT_ID),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .map_err(|e| match &e {
            ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => {
                "refresh_token 已失效（需重新登录 Kimi Code CLI）".to_string()
            }
            _ => format!("刷新请求失败（网络错误或服务不可用）: {e}"),
        })?;
    let body: OAuthTokenResponse = resp
        .into_json()
        .map_err(|e| format!("解析刷新响应失败: {e}"))?;
    let access_token = body
        .access_token
        .map(|t| t.trim().to_string())
        .filter(|t| !t.is_empty())
        .ok_or("刷新响应缺少 access_token")?;
    // expires_in 缺失按 CLI 实测值 900 秒兜底
    let expires_in_s = body.expires_in.unwrap_or(900).max(0);
    let expires_at_ms = chrono::Utc::now().timestamp_millis() + expires_in_s.saturating_mul(1000);
    Ok(OAuthTokenCache {
        access_token,
        expires_at_ms,
    })
}

/// 解析额度请求的 Bearer token（含 OAuth 内存续期），返回 (token, 是否
/// 用户显式配置的 API Key)——后者仅供 401 错误文案区分来源。优先级：
/// a. ~/.zbar/kimi.json 的 api_key（用户显式配置，长期有效，最高优先）；
/// b. 内存缓存的新 access_token（未过期）；
/// c. 凭据文件的 access_token（未过期 → 直接用并同步内存缓存；CLI 运行
///    期间会自己刷新文件里的 token）；
/// d. 文件 token 过期但有 refresh_token → POST 刷新端点换新（仅进内存
///    缓存，不写文件）；
/// e. 刷新失败（网络/HTTP 错误）→ 回退文件旧 access_token 试一次，由
///    /usages 的 401 分支给出最终指引。
/// 并发：锁只保护缓存读写，刷新 HTTP 在锁外执行，偶发重复刷新无害
/// （refresh_token 可复用）。
fn resolve_request_token() -> Result<(String, bool), String> {
    // a. 用户显式配置的 API Key
    match load_kimi_config() {
        Ok(cfg) => {
            let key = cfg.api_key.trim();
            if !key.is_empty() {
                return Ok((key.to_string(), true));
            }
        }
        Err(e) => {
            // 解析失败不静默：文件损坏/权限问题时应能从日志定位，
            // 仍继续走 OAuth 路径取 token
            eprintln!("[zbar-kimi] 读取 kimi.json 配置失败（跳过 API Key 分支）: {e}");
        }
    }

    let now_ms = chrono::Utc::now().timestamp_millis();

    // b. 内存缓存（上次刷新得到的新 token；锁内只做读取判断）
    {
        let cache = oauth_token_cache()
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = cache.as_ref() {
            if token_usable(cached.expires_at_ms, now_ms) {
                return Ok((cached.access_token.clone(), false));
            }
        }
    }

    match scan_cli_credentials()? {
        KimiCredential::Plain(token) => Ok((token, false)),
        KimiCredential::OAuth(oauth) => {
            // c. 文件 token 未过期 → 直接用并同步内存缓存
            if token_usable(oauth.expires_at_ms, now_ms) {
                store_oauth_cache(oauth.access_token.clone(), oauth.expires_at_ms);
                return Ok((oauth.access_token, false));
            }
            // d. 过期但有 refresh_token → 换新（只进内存，不写文件）
            match refresh_oauth_token(&oauth.refresh_token) {
                Ok(fresh) => {
                    let token = fresh.access_token.clone();
                    store_oauth_cache(token.clone(), fresh.expires_at_ms);
                    Ok((token, false))
                }
                Err(e) => {
                    // e. 刷新失败 → 回退旧 token 试一次
                    eprintln!(
                        "[zbar-kimi] OAuth token 自动续期失败（回退凭据文件 token 重试）: {e}"
                    );
                    Ok((oauth.access_token, false))
                }
            }
        }
    }
}

/// GET https://api.kimi.com/coding/v1/usages 的响应结构。
/// 顶层 usage 为总（周）额度；limits[] 为多窗口明细（如 5 小时窗口），按
/// window 折算时长归类；user.membership.level 为会员档位；boosterWallet 为
/// 加油包余额；totalQuota 为会员月总额度（实测当前普遍返回空对象 {}，
/// 复用 usage 块结构预埋解析，有值后自动生效）。全部字段 Option + 默认，
/// 结构不匹配时字段为 None（数值字段脏值也容错为 None，整体解析失败则
/// 报错字符串，不 panic）。
#[derive(Debug, Deserialize)]
struct KimiUsagesResponse {
    #[serde(default)]
    user: Option<KimiUser>,
    #[serde(default)]
    usage: Option<KimiUsageBlock>,
    #[serde(default)]
    limits: Vec<KimiLimitEntry>,
    #[serde(rename = "boosterWallet", default)]
    booster_wallet: Option<KimiBoosterWallet>,
    #[serde(rename = "totalQuota", default)]
    total_quota: Option<KimiUsageBlock>,
}

/// user 块：membership.level 为会员档位（如 "LEVEL_BASIC"）。
#[derive(Debug, Deserialize)]
struct KimiUser {
    #[serde(default)]
    membership: Option<KimiMembership>,
}

#[derive(Debug, Deserialize)]
struct KimiMembership {
    #[serde(default)]
    level: Option<String>,
}

/// usage / limits[].detail 共用的用量块：{limit, remaining, resetTime}。
/// 无 used 字段——已用 = limit - remaining；limit/remaining 实际为字符串
/// 数值（"100"，服务端 proto 风格 int64→string），双兼容数字。
#[derive(Debug, Deserialize)]
struct KimiUsageBlock {
    #[serde(default, deserialize_with = "deserialize_flexible_f64")]
    limit: Option<f64>,
    #[serde(default, deserialize_with = "deserialize_flexible_f64")]
    remaining: Option<f64>,
    #[serde(rename = "resetTime", default)]
    reset_time: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KimiLimitEntry {
    #[serde(default)]
    window: Option<KimiLimitWindow>,
    #[serde(default)]
    detail: Option<KimiUsageBlock>,
}

/// 窗口时长：duration（数字）+ timeUnit（枚举字符串，如 300
/// TIME_UNIT_MINUTE）。
#[derive(Debug, Deserialize)]
struct KimiLimitWindow {
    #[serde(default)]
    duration: Option<i64>,
    #[serde(rename = "timeUnit", default)]
    time_unit: Option<String>,
}

#[derive(Debug, Deserialize)]
struct KimiBoosterWallet {
    #[serde(default)]
    balance: Option<KimiBoosterBalance>,
    #[serde(
        rename = "monthlyUsed",
        default,
        deserialize_with = "deserialize_flexible_f64"
    )]
    monthly_used: Option<f64>,
}

#[derive(Debug, Deserialize)]
struct KimiBoosterBalance {
    #[serde(
        rename = "amountLeft",
        default,
        deserialize_with = "deserialize_flexible_f64"
    )]
    amount_left: Option<f64>,
}

/// 数值字段字符串/数字双兼容反序列化：/usages 的 limit/remaining 等字段为
/// proto 风格字符串数值（"100"），同名字段也可能直接是 JSON 数字；字符串
/// trim 后 parse::<f64>，数字直接取，其余类型（含 null）与解析失败一律
/// 容错为 None——单个字段脏值绝不让整体反序列化报错。
fn deserialize_flexible_f64<'de, D>(deserializer: D) -> Result<Option<f64>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    Ok(match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    })
}

/// 窗口时长折算为秒（timeUnit 兼容服务端枚举值 TIME_UNIT_MINUTE 与简写
/// MINUTE/MINUTES 等大小写变体）。无法折算返回 None。
fn window_seconds(window: &KimiLimitWindow) -> Option<i64> {
    let duration = window.duration?;
    let unit = window.time_unit.as_deref()?.trim().to_ascii_uppercase();
    let mult: i64 = match unit.as_str() {
        "TIME_UNIT_SECOND" | "SECOND" | "SECONDS" => 1,
        "TIME_UNIT_MINUTE" | "MINUTE" | "MINUTES" => 60,
        "TIME_UNIT_HOUR" | "HOUR" | "HOURS" => 3_600,
        "TIME_UNIT_DAY" | "DAY" | "DAYS" => 86_400,
        "TIME_UNIT_WEEK" | "WEEK" | "WEEKS" => 604_800,
        _ => return None,
    };
    Some(duration.saturating_mul(mult))
}

/// resetTime（RFC3339 字符串，如 "2026-08-28T09:58:45.362281Z"）→ 毫秒
/// 时间戳；微秒精度自然截断到毫秒；解析失败返回 None。
fn parse_reset_time_ms(s: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(s.trim())
        .ok()
        .map(|t| t.timestamp_millis())
}

/// (limit-remaining)/limit → 已用百分比（0-100）。limit/remaining 缺失或
/// limit 非正值时返回 None（防除零）。
fn pct_from_limit_remaining(limit: Option<f64>, remaining: Option<f64>) -> Option<f64> {
    let limit = limit?;
    let remaining = remaining?;
    if limit <= 0.0 {
        return None;
    }
    Some((limit - remaining) / limit * 100.0)
}

/// 响应 → 前端展示结构（纯函数，便于单测）：
/// - limits[] 按 window 折算秒数归类，≥2 天 → 周窗口槽，其余 → 5 小时窗口槽
///   （同槽后出现的覆盖先出现的，参照 codex.rs 的 classify 思路）；
/// - 顶层 usage 为周额度（权威口径），存在时覆盖 weekly 槽；
/// - totalQuota（会员月总额度）→ monthly 槽，空对象自然解析为全 None；
/// - 任一窗口或加油包字段有值才返回 Some，否则 None（额度块不展示）。
fn rate_limits_from_response(resp: &KimiUsagesResponse) -> Option<KimiRateLimits> {
    let mut hour5: Option<(Option<f64>, Option<i64>)> = None;
    let mut weekly: Option<(Option<f64>, Option<i64>)> = None;
    for entry in &resp.limits {
        let (Some(window), Some(detail)) = (entry.window.as_ref(), entry.detail.as_ref()) else {
            continue;
        };
        let slot = (
            pct_from_limit_remaining(detail.limit, detail.remaining),
            detail.reset_time.as_deref().and_then(parse_reset_time_ms),
        );
        if window_seconds(window).unwrap_or(0) >= 2 * 86_400 {
            weekly = Some(slot);
        } else {
            hour5 = Some(slot);
        }
    }
    if let Some(usage) = resp.usage.as_ref() {
        let slot = (
            pct_from_limit_remaining(usage.limit, usage.remaining),
            usage.reset_time.as_deref().and_then(parse_reset_time_ms),
        );
        if slot.0.is_some() || slot.1.is_some() {
            weekly = Some(slot);
        }
    }

    let plan_type = resp
        .user
        .as_ref()
        .and_then(|u| u.membership.as_ref())
        .and_then(|m| m.level.clone());
    let primary_pct = hour5.as_ref().and_then(|s| s.0);
    let primary_reset_at = hour5.as_ref().and_then(|s| s.1);
    let secondary_pct = weekly.as_ref().and_then(|s| s.0);
    let secondary_reset_at = weekly.as_ref().and_then(|s| s.1);
    let booster_balance = resp
        .booster_wallet
        .as_ref()
        .and_then(|w| w.balance.as_ref())
        .and_then(|b| b.amount_left);
    let booster_monthly_used = resp.booster_wallet.as_ref().and_then(|w| w.monthly_used);
    let monthly_pct = resp
        .total_quota
        .as_ref()
        .and_then(|q| pct_from_limit_remaining(q.limit, q.remaining));
    let monthly_reset_at = resp
        .total_quota
        .as_ref()
        .and_then(|q| q.reset_time.as_deref())
        .and_then(parse_reset_time_ms);

    if primary_pct.is_some()
        || secondary_pct.is_some()
        || booster_balance.is_some()
        || monthly_pct.is_some()
    {
        Some(KimiRateLimits {
            plan_type,
            primary_pct,
            primary_reset_at,
            secondary_pct,
            secondary_reset_at,
            booster_balance,
            booster_monthly_used,
            monthly_pct,
            monthly_reset_at,
        })
    } else {
        None
    }
}

/// 实时额度结果缓存（成功 60s / 失败 15s 双 TTL，照 claude.rs 模式）。
/// 成功缓存：前端多命令高频触发，防止打爆端点。失败负缓存：无凭据/网络
/// 不通时同样会被高频触发，短暂缓存失败结果避免重试风暴。
static LIVE_LIMITS_CACHE: OnceLock<
    Mutex<Option<(std::time::Instant, Result<Option<KimiRateLimits>, String>)>>,
> = OnceLock::new();

/// 拉取 Kimi 订阅额度（带缓存版，供诊断/测试用）。
#[allow(dead_code)]
pub fn fetch_live_rate_limits() -> Result<Option<KimiRateLimits>, String> {
    fetch_live_rate_limits_with_freshness().map(|(limits, _)| limits)
}

/// 拉取实时额度并标记本次结果是否来自新的 HTTP 请求。
/// 缓存命中仍可用于当前进度展示，但不应作为新的历史采样。
pub fn fetch_live_rate_limits_with_freshness(
) -> Result<(Option<KimiRateLimits>, bool), String> {
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

// ===== 官方会员档位名（GET {api_base}/me，档位展示优化）=====

/// GET {api_base}/me 的响应结构（实测返回 user_id/nickname/user_level/
/// user_level_name/region 等）。只关心 user_level_name——官方会员档位名
/// （Adagio/Andante/Moderato/Allegretto/Allegro）。字段全 Option + serde
/// default，缺字段不报错；user_level_name 类型不符（脏值）时整体解析失败，
/// 由 parse_user_level_name 容错为 None。
#[derive(Debug, Deserialize)]
struct MeResponse {
    #[serde(default)]
    user_level_name: Option<String>,
}

/// 解析 /me 响应体中的官方会员档位名（纯函数，便于单测）：
/// user_level_name trim 后非空才返回；字段缺失/为空/类型脏值/整体非法 JSON
/// 一律 None。与 user.membership.level 的区别：前者是官方对外的会员档位名
/// （如 "Moderato"），后者是 usages 接口的内部枚举（如 "LEVEL_BASIC"），用户
/// 看不懂，故仅作回退。
fn parse_user_level_name(body: &str) -> Option<String> {
    let resp: MeResponse = serde_json::from_str(body).ok()?;
    resp.user_level_name
        .map(|n| n.trim().to_string())
        .filter(|n| !n.is_empty())
}

/// 档位名缓存 TTL：10 分钟。成功与失败统一缓存——官方档位极少变化，无需每次
/// 刷新额度都打一次 /me；401（OAuth 过期）/网络故障同样进负缓存，刷新 OAuth
/// 后最迟 10 分钟自动恢复显示官方名，期间回退 membership.level，不值得频繁重试。
const USER_LEVEL_NAME_CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(600);

static USER_LEVEL_NAME_CACHE: OnceLock<Mutex<Option<(std::time::Instant, Option<String>)>>> =
    OnceLock::new();

fn user_level_name_cache()
-> &'static Mutex<Option<(std::time::Instant, Option<String>)>> {
    USER_LEVEL_NAME_CACHE.get_or_init(|| Mutex::new(None))
}

/// 获取官方会员档位名（GET {api_base}/me 的 user_level_name，带 10 分钟缓存）。
/// 任何失败（无凭据 / 401/403 / 网络错误 / 解析失败 / 字段为空）一律返回 None，
/// 由调用方回退 user.membership.level——本接口只为档位展示可读性服务，绝不做
/// 阻断性诊断依据；401 时也不清 OAuth 缓存（usages 请求会自行处理凭据失效）。
fn fetch_user_level_name() -> Option<String> {
    // 缓存命中：成功与失败同按 10 分钟 TTL 复用（锁内只做读取判断）
    {
        let guard = user_level_name_cache()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if let Some((at, val)) = guard.as_ref() {
            if at.elapsed() < USER_LEVEL_NAME_CACHE_TTL {
                return val.clone();
            }
        }
    }

    let fetched: Option<String> = (|| {
        let (token, _) = resolve_request_token().ok()?;
        let endpoints = kimi_endpoints();
        match build_kimi_agent()
            .get(&format!("{}/me", endpoints.api_base))
            .set("Authorization", &format!("Bearer {token}"))
            .set("Accept", "application/json")
            .call()
        {
            Ok(resp) => match resp.into_string() {
                Ok(body) => parse_user_level_name(&body),
                Err(e) => {
                    eprintln!("[zbar-kimi] 读取 /me 响应体失败（回退 usages 枚举档位）: {e}");
                    None
                }
            },
            Err(e) => {
                eprintln!("[zbar-kimi] 获取官方会员档位失败（回退 usages 枚举档位）: {e}");
                None
            }
        }
    })();

    *user_level_name_cache()
        .lock()
        .unwrap_or_else(|p| p.into_inner()) = Some((std::time::Instant::now(), fetched.clone()));
    fetched
}

fn fetch_live_rate_limits_uncached() -> Result<Option<KimiRateLimits>, String> {
    let (token, from_api_key) = resolve_request_token()?;
    let endpoints = kimi_endpoints();
    let agent = build_kimi_agent();

    let resp_result = agent
        .get(&format!("{}/usages", endpoints.api_base))
        .set("Authorization", &format!("Bearer {token}"))
        .set("Accept", "application/json")
        .call();
    let resp = resp_result.map_err(|e| match &e {
        // 鉴权失败按凭据来源给出准确指引（不一律引导配 API Key）
        ureq::Error::Status(401, _) | ureq::Error::Status(403, _) => {
            if from_api_key {
                "Kimi 额度接口鉴权失败：设置的 API Key 无效或无权限，请检查 ~/.zbar/kimi.json 中的 Kimi API Key".to_string()
            } else {
                // OAuth token 被服务端拒绝：清空内存缓存，下次取 token 改走
                // 重读凭据文件/刷新路径，避免无效 token 在缓存 TTL 内反复 401
                clear_oauth_cache();
                "Kimi 额度接口鉴权失败（OAuth 凭据已过期且自动续期失败）：请运行一次 kimi 命令刷新登录，或在设置中配置长期 API Key".to_string()
            }
        }
        _ => format!("Kimi 实时额度请求失败（网络错误或服务不可用）: {e}"),
    })?;
    let usages: KimiUsagesResponse = resp
        .into_json()
        .map_err(|e| format!("解析实时额度失败: {e}"))?;
    let mut limits = rate_limits_from_response(&usages);
    // 档位合并：官方会员档位名（/me 的 user_level_name）优先，获取失败回退
    // user.membership.level。rate_limits_from_response 保持纯函数不动（单测
    // 直接断言其原始输出），合并只发生在真实调用侧。
    if let Some(entry) = limits.as_mut() {
        entry.plan_type = fetch_user_level_name().or(entry.plan_type.take());
    }
    Ok(limits)
}

// ===== 诊断 =====

/// Kimi 诊断信息（排查"无数据"问题）
#[derive(Debug, Clone, Serialize)]
pub struct KimiDebugInfo {
    /// sessions 目录路径
    pub sessions_dir: String,
    /// 目录是否存在
    pub sessions_dir_exists: bool,
    /// 目录下 wire.jsonl 文件数
    pub session_files: usize,
    /// 导入库累计记录数
    pub imported_records: i64,
    /// 最新一条用量的时间（毫秒）
    pub latest_session_ms: Option<i64>,
}

/// 诊断信息（get_claude_debug 同款用途）。导入失败不阻断——目录不存在时
/// 恰恰要靠这些字段定位问题。诊断必须真实执行导入（绕过节流）。
pub fn debug_info() -> Result<KimiDebugInfo, String> {
    if let Err(e) = import_incremental_force() {
        eprintln!("[zbar-kimi] 诊断时增量导入失败: {e}");
    }

    let dir = sessions_dir_path();
    let mut files = Vec::new();
    if dir.is_dir() {
        collect_session_files(&dir, 6, &mut files);
    }

    let (imported_records, latest_session_ms) = open_kimi_db()
        .map(|conn| {
            conn.query_row(
                "SELECT COUNT(*), MAX(started_at) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap_or((0, None))
        })
        .unwrap_or((0, None));

    Ok(KimiDebugInfo {
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

    /// usage.record 字段映射与 computed_total：四字段映射到 zcode 口径，
    /// 缺失字段按 0 容错，总量为四项之和。
    #[test]
    fn usage_record_field_mapping_and_total() {
        let line: WireLine = serde_json::from_str(
            r#"{"type":"usage.record","model":"kimi-k2-0905","time":1756000000000,
                "usage":{"inputOther":100,"output":50,"inputCacheRead":200,"inputCacheCreation":30}}"#,
        )
        .expect("usage.record 解析失败");
        assert_eq!(line.line_type.as_deref(), Some("usage.record"));
        assert_eq!(line.model.as_deref(), Some("kimi-k2-0905"));
        assert_eq!(line.time, Some(1_756_000_000_000));
        let usage = line.usage.expect("usage 缺失");
        assert_eq!(usage.input_other, 100);
        assert_eq!(usage.output, 50);
        assert_eq!(usage.input_cache_read, 200);
        assert_eq!(usage.input_cache_creation, 30);
        assert_eq!(usage.computed_total(), 380);

        // 部分字段缺失：缺省按 0，总量为剩余项之和
        let partial: WireLine = serde_json::from_str(
            r#"{"type":"usage.record","model":"m","time":1,"usage":{"output":7}}"#,
        )
        .expect("部分字段解析失败");
        assert_eq!(partial.usage.unwrap().computed_total(), 7);
    }

    /// session_id 路径回溯：严格三层标准布局取 sessionId 目录名；嵌套
    /// （agents 不在 sessions 根下一层）与无 agents 层的结构一律全路径兜底，
    /// 防止不同会话的同名中间目录共享 session_id。
    #[test]
    fn session_id_from_wire_path() {
        let root = Path::new("/h/.kimi-code/sessions");
        // 标准布局 root/<sessionId>/agents/<agentId>/wire.jsonl
        let std = root.join("sess-a").join("agents").join("ag-1").join("wire.jsonl");
        assert_eq!(session_id_from_path(&std, root), "sess-a");
        // 嵌套布局：回溯会取到中间目录 sub → 必须全路径兜底
        let nested = root.join("sess-b").join("sub").join("agents").join("ag").join("wire.jsonl");
        assert_eq!(
            session_id_from_path(&nested, root),
            nested.to_string_lossy().to_string()
        );
        // 无 agents 层 → 全路径兜底（保证唯一）
        let flat = root.join("sess-c").join("wire.jsonl");
        assert_eq!(
            session_id_from_path(&flat, root),
            flat.to_string_lossy().to_string()
        );
    }

    /// usages 响应（真实线上结构，回归：旧结构按数字 used/resetTime 解析，
    /// 遇字符串 "100" 直接整体报错）：字符串数值、RFC3339 resetTime、
    /// user.membership.level 档位、300 分钟窗口 → 5h 槽、顶层 usage → 周槽。
    #[test]
    fn usages_response_window_classification_and_reset_time() {
        let resp: KimiUsagesResponse = serde_json::from_str(
            r#"{
                "user": {"region":"REGION_CN","membership":{"level":"LEVEL_BASIC"},"businessId":""},
                "usage": {"limit":"100","remaining":"100","resetTime":"2026-08-28T09:58:45.362281Z"},
                "limits": [
                    {"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                     "detail":{"limit":"100","remaining":"100","resetTime":"2026-08-27T10:58:45.362281Z"}}
                ],
                "parallel": {"limit":"10"},
                "totalQuota": {},
                "authentication": {"method":"METHOD_ACCESS_TOKEN","scope":"FEATURE_CODING"},
                "subType": "TYPE_PURCHASE",
                "domain": "DOMAIN_NEXUS"
            }"#,
        )
        .expect("响应解析失败");
        let limits = rate_limits_from_response(&resp).expect("应组装出额度");
        // 档位来自 user.membership.level
        assert_eq!(limits.plan_type.as_deref(), Some("LEVEL_BASIC"));
        // 300 分钟 = 5 小时 → primary：100 剩 100 → 已用 0%
        assert_eq!(limits.primary_pct, Some(0.0));
        assert_eq!(limits.primary_reset_at, Some(1_787_828_325_362));
        // 顶层 usage → weekly 槽：同样 0%，resetTime 为次日（微秒截断到毫秒 362）
        assert_eq!(limits.secondary_pct, Some(0.0));
        assert_eq!(limits.secondary_reset_at, Some(1_787_911_125_362));
        // 该账号无加油包 → None
        assert_eq!(limits.booster_balance, None);
        assert_eq!(limits.booster_monthly_used, None);
        // totalQuota 为空对象 {}（服务端当前普遍形态）→ monthly 全 None
        assert_eq!(limits.monthly_pct, None);
        assert_eq!(limits.monthly_reset_at, None);

        // 窗口归类 + 已用计算：5 小时用掉 75（100 剩 25 → 75%），
        // 7 天 TIME_UNIT_DAY ≥ 2 天 → 周槽；数字型 limit/remaining 同样可解析
        let classified: KimiUsagesResponse = serde_json::from_str(
            r#"{
                "usage": null,
                "limits": [
                    {"window":{"duration":300,"timeUnit":"TIME_UNIT_MINUTE"},
                     "detail":{"limit":"100","remaining":"25","resetTime":"2026-08-27T10:58:45Z"}},
                    {"window":{"duration":7,"timeUnit":"TIME_UNIT_DAY"},
                     "detail":{"limit":200,"remaining":150,"resetTime":"2026-08-31T00:00:00.5Z"}}
                ]
            }"#,
        )
        .expect("混合数值解析失败");
        let mixed = rate_limits_from_response(&classified).expect("应组装出额度");
        assert_eq!(mixed.plan_type, None);
        assert_eq!(mixed.primary_pct, Some(75.0));
        assert_eq!(mixed.primary_reset_at, Some(1_787_828_325_000));
        // 周槽来自 limits（无顶层 usage）：(200-150)/200 = 25%，500µs → 500ms
        assert_eq!(mixed.secondary_pct, Some(25.0));
        assert_eq!(mixed.secondary_reset_at, Some(1_788_134_400_500));

        // 全空响应 → None（额度块不展示）
        let empty: KimiUsagesResponse =
            serde_json::from_str("{}").expect("空响应解析失败");
        assert!(rate_limits_from_response(&empty).is_none());
    }

    /// totalQuota（会员月总额度）预埋解析：有值时按 usage 块同款口径计算
    /// pct/reset；仅 totalQuota 有值即可独立让额度块成立（Some 判定含
    /// monthly_pct）；字段缺失或 limit 脏值 → None，额度块不成立。
    #[test]
    fn usages_total_quota_monthly_slot() {
        // 有值样例：结构与 usage 块相同（字符串数值 + RFC3339 resetTime），
        // (1000-250)/1000 = 75%，resetTime → 毫秒时间戳
        let resp: KimiUsagesResponse = serde_json::from_str(
            r#"{
                "totalQuota": {"limit":"1000","remaining":"250","resetTime":"2026-09-01T00:00:00Z"}
            }"#,
        )
        .expect("totalQuota 响应解析失败");
        let limits = rate_limits_from_response(&resp).expect("仅 totalQuota 有值也应组装出额度");
        assert_eq!(limits.monthly_pct, Some(75.0));
        assert_eq!(limits.monthly_reset_at, Some(1_788_220_800_000));
        // 其余窗口未提供 → None（monthly 不污染既有槽位）
        assert_eq!(limits.primary_pct, None);
        assert_eq!(limits.primary_reset_at, None);
        assert_eq!(limits.secondary_pct, None);
        assert_eq!(limits.secondary_reset_at, None);

        // totalQuota 字段缺失 → monthly 全 None，额度块不成立
        let missing: KimiUsagesResponse =
            serde_json::from_str(r#"{"usage": null, "limits": []}"#).expect("缺失样例解析失败");
        assert!(rate_limits_from_response(&missing).is_none());

        // limit 脏值（非法字符串）→ pct None；resetTime 单独有值不算数，
        // 额度块不成立（与 primary/secondary 只认 pct 的口径一致）
        let dirty: KimiUsagesResponse = serde_json::from_str(
            r#"{"totalQuota": {"limit":"abc","remaining":10,"resetTime":"2026-09-01T00:00:00Z"}}"#,
        )
        .expect("脏值应容错为 None 而非报错");
        assert!(rate_limits_from_response(&dirty).is_none());
    }

    /// 数值字段字符串/数字双兼容：字符串 trim 后可解析、JSON 数字直取、
    /// 非法字符串/null/bool 容错为 None（不得让整体反序列化报错）。
    #[test]
    fn usages_flexible_number_fields() {
        let block: KimiUsageBlock = serde_json::from_str(
            r#"{"limit":" 100 ","remaining":42.5,"resetTime":"2026-08-27T10:58:45.362281Z"}"#,
        )
        .expect("字符串/数字混合应解析成功");
        assert_eq!(block.limit, Some(100.0));
        assert_eq!(block.remaining, Some(42.5));
        assert_eq!(block.reset_time.as_deref(), Some("2026-08-27T10:58:45.362281Z"));

        // 脏值容错：非法字符串 / null / 布尔 → None，不报错
        let dirty: KimiUsageBlock = serde_json::from_str(
            r#"{"limit":"abc","remaining":null,"resetTime":null}"#,
        )
        .expect("脏值应容错为 None 而非报错");
        assert_eq!(dirty.limit, None);
        assert_eq!(dirty.remaining, None);
        assert_eq!(dirty.reset_time, None);
        let bool_block: KimiUsageBlock =
            serde_json::from_str(r#"{"limit":true}"#).expect("布尔也应容错");
        assert_eq!(bool_block.limit, None);

        // limit<=0 → 百分比 None（防除零）；正常值 (100-30)/100 = 70%
        assert_eq!(pct_from_limit_remaining(Some(0.0), Some(0.0)), None);
        assert_eq!(pct_from_limit_remaining(None, Some(30.0)), None);
        assert_eq!(pct_from_limit_remaining(Some(100.0), None), None);
        assert_eq!(pct_from_limit_remaining(Some(100.0), Some(30.0)), Some(70.0));
    }

    /// RFC3339 resetTime 解析：微秒截断到毫秒、无小数秒、非法输入 None、
    /// 非零时区偏移按绝对时间折算。
    #[test]
    fn reset_time_rfc3339_parsing() {
        // .362281Z（微秒）→ 362 毫秒
        assert_eq!(
            parse_reset_time_ms("2026-08-27T10:58:45.362281Z"),
            Some(1_787_828_325_362)
        );
        // 无小数秒 → 整秒
        assert_eq!(
            parse_reset_time_ms("2026-08-27T10:58:45Z"),
            Some(1_787_828_325_000)
        );
        // 带空白 → trim 后仍可解析
        assert_eq!(
            parse_reset_time_ms(" 2026-08-27T10:58:45Z "),
            Some(1_787_828_325_000)
        );
        // 非零时区偏移：+08:00 的 18:58:45 与 UTC 10:58:45 同一时刻
        assert_eq!(
            parse_reset_time_ms("2026-08-27T18:58:45+08:00"),
            Some(1_787_828_325_000)
        );
        // 非法输入 → None
        assert_eq!(parse_reset_time_ms("not-a-time"), None);
        assert_eq!(parse_reset_time_ms(""), None);
        assert_eq!(parse_reset_time_ms("1756200000"), None);
    }

    /// 凭据多键名容错探测：五种键名依次生效，空串/非字符串/未知键跳过。
    #[test]
    fn credentials_token_multi_key_probe() {
        let probe = |json: &str| -> Option<String> {
            let v: serde_json::Value = serde_json::from_str(json).unwrap();
            extract_token_from_value(&v)
        };
        assert_eq!(probe(r#"{"apiKey":"k-1"}"#).as_deref(), Some("k-1"));
        assert_eq!(probe(r#"{"api_key":"k-2"}"#).as_deref(), Some("k-2"));
        assert_eq!(probe(r#"{"accessToken":"t-3"}"#).as_deref(), Some("t-3"));
        assert_eq!(probe(r#"{"access_token":"t-4"}"#).as_deref(), Some("t-4"));
        assert_eq!(probe(r#"{"token":"t-5"}"#).as_deref(), Some("t-5"));
        // 多键并存：按探测顺序取先出现者
        assert_eq!(
            probe(r#"{"token":"t","apiKey":"k"}"#).as_deref(),
            Some("k")
        );
        // 空串 / 非字符串 / 未知键 → None（trim 后为空同样跳过）
        assert_eq!(probe(r#"{"apiKey":""}"#), None);
        assert_eq!(probe(r#"{"apiKey":"  "}"#), None);
        assert_eq!(probe(r#"{"apiKey":123}"#), None);
        assert_eq!(probe(r#"{"other":"x"}"#), None);
    }

    /// OAuth 凭据识别：access_token + expires_at + refresh_token 三字段
    /// 齐全才视为 OAuth 模式（expires_at 兼容秒/毫秒），任一缺失/为空/
    /// 类型不符均降级（返回 None，走多键名探测）。
    #[test]
    fn oauth_credential_detection_and_expires_unit() {
        let parse = |json: &str| {
            parse_oauth_credential(&serde_json::from_str::<serde_json::Value>(json).unwrap())
        };
        // 三字段齐全（CLI 登录后的真实结构，expires_at 为秒）→ 折算毫秒
        let cred = parse(
            r#"{"access_token":"at","refresh_token":"rt","expires_at":1756000000,
                "expires_in":900,"token_type":"Bearer","scope":"openid"}"#,
        )
        .expect("三字段齐全应识别为 OAuth");
        assert_eq!(cred.access_token, "at");
        assert_eq!(cred.refresh_token, "rt");
        assert_eq!(cred.expires_at_ms, 1_756_000_000_000);
        // expires_at 为毫秒 → 原样保留
        let cred_ms =
            parse(r#"{"access_token":"at","refresh_token":"rt","expires_at":1756000900000}"#)
                .expect("毫秒 expires_at 应识别");
        assert_eq!(cred_ms.expires_at_ms, 1_756_000_900_000);
        // 字段带空白 → trim 后仍可用
        let cred_pad =
            parse(r#"{"access_token":" at ","refresh_token":" rt ","expires_at":1756000000}"#)
                .expect("带空白字段应识别");
        assert_eq!(cred_pad.access_token, "at");
        assert_eq!(cred_pad.refresh_token, "rt");
        // 缺任一字段 → 降级（非 OAuth）
        assert!(
            parse(r#"{"access_token":"at","expires_at":123}"#).is_none(),
            "缺 refresh_token 应降级"
        );
        assert!(
            parse(r#"{"refresh_token":"rt","expires_at":123}"#).is_none(),
            "缺 access_token 应降级"
        );
        assert!(
            parse(r#"{"access_token":"at","refresh_token":"rt"}"#).is_none(),
            "缺 expires_at 应降级"
        );
        // 空串 / 非字符串 / expires_at 非整数 → 降级
        assert!(parse(r#"{"access_token":"","refresh_token":"rt","expires_at":123}"#).is_none());
        assert!(parse(r#"{"access_token":"at","refresh_token":"  ","expires_at":123}"#).is_none());
        assert!(parse(r#"{"access_token":"at","refresh_token":"rt","expires_at":"123"}"#).is_none());
        // 纯 apiKey 文件 → 非 OAuth
        assert!(parse(r#"{"apiKey":"k-1"}"#).is_none());
    }

    /// expires_at 单位判别边界：秒 ×1000、毫秒原样、0 与负值安全折算。
    #[test]
    fn expires_at_unit_detection() {
        assert_eq!(normalize_expires_at_ms(1_756_000_000), 1_756_000_000_000); // 秒
        assert_eq!(normalize_expires_at_ms(1_756_000_000_000), 1_756_000_000_000); // 毫秒
        assert_eq!(normalize_expires_at_ms(0), 0);
        assert_eq!(normalize_expires_at_ms(-5), -5_000);
    }

    /// region 域名分流：global → .ai；mainland-cn/缺失/未知值 → 默认 .com。
    #[test]
    fn region_endpoint_routing() {
        let global = endpoints_for_region("global");
        assert_eq!(global.oauth_host, "https://auth.kimi.ai");
        assert_eq!(global.api_base, "https://api.kimi.ai/coding/v1");
        // 带空白 trim 后仍识别为 global
        assert_eq!(
            endpoints_for_region("  global\n").oauth_host,
            "https://auth.kimi.ai"
        );
        // 大陆 / 缺失（空串）/ 未知值 → 默认 .com
        for region in ["mainland-cn", "", "unknown-region", "GLOBAL"] {
            let ep = endpoints_for_region(region);
            assert_eq!(ep.oauth_host, "https://auth.kimi.com", "region={region}");
            assert_eq!(ep.api_base, "https://api.kimi.com/coding/v1", "region={region}");
        }
    }

    /// token 可用性判别：60 秒提前量（剩余寿命 > 60s 才可用）。
    #[test]
    fn token_usable_60s_lead_time() {
        let now = 1_000_000_000_000i64;
        assert!(token_usable(now + 60_001, now), "剩余 > 60s 可用");
        assert!(!token_usable(now + 60_000, now), "剩余 = 60s 不可用（提前量边界）");
        assert!(!token_usable(now + 59_999, now), "剩余 < 60s 不可用");
        assert!(!token_usable(now - 1, now), "已过期不可用");
    }

    /// /me 响应解析出官方会员档位名：正常值原样返回、带空白 trim、空串/纯
    /// 空白/字段缺失/类型脏值/非法 JSON/空响应体一律 None（不 panic）。
    #[test]
    fn me_response_user_level_name_parsing() {
        // 正常：实测响应结构，user_level_name 即官方档位名
        assert_eq!(
            parse_user_level_name(
                r#"{"user_id":"u-1","nickname":"n","user_level":20,
                    "user_level_name":"Moderato","region":"REGION_CN"}"#,
            ),
            Some("Moderato".to_string())
        );
        // 带空白 → trim 后返回
        assert_eq!(
            parse_user_level_name(r#"{"user_level_name":"  Allegretto  ","region":"REGION_CN"}"#),
            Some("Allegretto".to_string())
        );
        // 空串 / 纯空白 → None
        assert_eq!(parse_user_level_name(r#"{"user_level_name":""}"#), None);
        assert_eq!(parse_user_level_name(r#"{"user_level_name":"   "}"#), None);
        // 字段缺失 → None
        assert_eq!(parse_user_level_name(r#"{"user_id":"u-1"}"#), None);
        // 类型脏值（数字而非字符串）→ 整体解析降级 None
        assert_eq!(parse_user_level_name(r#"{"user_level_name":20}"#), None);
        // 非法 JSON / 空响应体 → None
        assert_eq!(parse_user_level_name("not-json"), None);
        assert_eq!(parse_user_level_name(""), None);
    }

    /// 临时库计数的辅助断言：(记录数, computed_total 合计)。
    fn usage_count(conn: &Connection) -> (i64, i64) {
        conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(computed_total_tokens), 0) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("临时库计数查询失败")
    }

    /// 临时库 duration 的辅助断言：按 started_at 升序返回 duration_ms 列表。
    fn durations(conn: &Connection) -> Vec<Option<i64>> {
        conn.prepare("SELECT duration_ms FROM model_usage ORDER BY started_at ASC")
            .expect("准备 duration 查询失败")
            .query_map([], |row| row.get(0))
            .expect("读取 duration 失败")
            .filter_map(|r| r.ok())
            .collect()
    }

    /// wire.jsonl 导入幂等冒烟：临时目录造样例文件 + 临时 sqlite（不依赖真实
    /// ~/.kimi-code 与网络，也不触碰 ~/.zbar/kimi.sqlite），干扰行全部跳过；
    /// 重复导入记录数不变；追加行后序号续读不重复。
    #[test]
    fn wire_import_idempotent_smoke() {
        let tmp = std::env::temp_dir().join(format!("zbar-kimi-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sessions_root = tmp.join("sessions");
        let db_path = tmp.join("kimi-test.sqlite");
        let wire_dir = sessions_root.join("sess-001").join("agents").join("ag-1");
        std::fs::create_dir_all(&wire_dir).expect("创建样例目录失败");
        std::fs::write(
            wire_dir.join("wire.jsonl"),
            concat!(
                r#"{"type":"metadata","protocolVersion":1,"sessionId":"sess-001"}"#,
                "\n",
                r#"{"type":"llm.request","model":"kimi-k2","time":1755999936000,"usage":{"inputOther":999}}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2-0905","time":1756000000000,"usage":{"inputOther":100,"output":50,"inputCacheRead":200,"inputCacheCreation":30}}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2-thinking","time":1756000060000,"usage":{"inputOther":10,"output":5}}"#,
                "\n",
                r#"{"type":"usage.record","model":"","time":1756000120000,"usage":{"inputOther":1,"output":1}}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2-0905","usage":{"inputOther":7,"output":7}}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2-0905","time":1756000180000,"usage":{"inputOther":0,"output":0}}"#,
                "\n",
                "不是 JSON 的脏行\n",
            ),
        )
        .expect("写入样例文件失败");

        // 注入临时 sessions 目录与临时库（参数化内核，无全局状态）
        import_incremental_into(&sessions_root, &db_path).expect("增量导入失败");
        // 干扰行（metadata/llm.request/空模型/缺 time/零值/脏行）全部跳过
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        assert_eq!(usage_count(&conn), (2, 380 + 15), "有效 usage.record 应为 2 条");
        // session_id 路径回溯 + provider 归属
        let sessions: Vec<String> = conn
            .prepare("SELECT DISTINCT session_id FROM model_usage")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(sessions, vec!["sess-001".to_string()], "session 提取异常");
        let providers: Vec<String> = conn
            .prepare("SELECT DISTINCT provider_id FROM model_usage")
            .unwrap()
            .query_map([], |row| row.get::<_, String>(0))
            .unwrap()
            .filter_map(|r| r.ok())
            .collect();
        assert_eq!(providers, vec!["kimi".to_string()], "provider 归属异常");
        // 耗时落库：前置 llm.request time=1755999936000，两条 usage.record
        // 均与"最近一条 llm.request"配对（64000 / 124000ms，都在合理区间）
        assert_eq!(
            durations(&conn),
            vec![Some(64_000), Some(124_000)],
            "duration 应按事件时间差落库"
        );

        // 幂等：再导一次，记录数不变
        import_incremental_into(&sessions_root, &db_path).expect("二次导入失败");
        assert_eq!(usage_count(&conn), (2, 380 + 15), "重复导入导致记录数变化");
        assert_eq!(
            durations(&conn),
            vec![Some(64_000), Some(124_000)],
            "重复导入不得改动 duration"
        );

        // 追加新行：续读只解析新增部分，序号从该文件已入库最大值恢复，不重复
        let wire_path = wire_dir.join("wire.jsonl");
        let mut appended = std::fs::read_to_string(&wire_path).unwrap();
        appended.push_str(
            r#"{"type":"usage.record","model":"kimi-k2-0905","time":1756000240000,"usage":{"inputOther":8,"output":8}}"#,
        );
        appended.push('\n');
        std::fs::write(&wire_path, appended).expect("追加样例行失败");
        import_incremental_into(&sessions_root, &db_path).expect("追加后导入失败");
        assert_eq!(usage_count(&conn), (3, 380 + 15 + 16), "追加 1 行后应为 3 条");
        // 追加行 time=1756000240000：续读窗口内无 llm.request（请求在已读
        // 部分）→ 该行 duration NULL；已入库两行的旧值不受影响
        assert_eq!(
            durations(&conn),
            vec![Some(64_000), Some(124_000), None],
            "续读窗口内无 llm.request 的新行 duration 应为 NULL"
        );
        // 再导一次仍幂等
        import_incremental_into(&sessions_root, &db_path).expect("追加重放导入失败");
        assert_eq!(usage_count(&conn), (3, 380 + 15 + 16), "追加重放导致重复计数");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 同一 session 多个 agent 文件的序号隔离（回归：dedupe_key 无文件维度时，
    /// 第二个文件首导从 0 重计序号会与第一个文件撞键，"总量更大者胜"静默丢
    /// 数据）+ 文件重写 reparse 路径 + 同键 upsert 冲突的双向行为
    /// （大值覆盖 / 小值不覆盖）。
    #[test]
    fn wire_import_multi_agent_and_reparse() {
        let tmp = std::env::temp_dir().join(format!("zbar-kimi-multi-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sessions_root = tmp.join("sessions");
        let db_path = tmp.join("kimi-multi.sqlite");
        let main_dir = sessions_root.join("sess-9").join("agents").join("main");
        let sub_dir = sessions_root.join("sess-9").join("agents").join("researcher");
        std::fs::create_dir_all(&main_dir).expect("创建 main 目录失败");
        std::fs::create_dir_all(&sub_dir).expect("创建 researcher 目录失败");
        let wire = |dir: &Path, lines: &[&str]| {
            let mut data = lines.join("\n");
            data.push('\n');
            std::fs::write(dir.join("wire.jsonl"), data).expect("写入样例文件失败");
        };

        // 第 1 步：main 两条（100 / 50），researcher 一条（15）——同一 session
        // 两个 agent 文件，序号各自从 1 计，不得互相吞并
        wire(&main_dir, &[
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000000000,"usage":{"inputOther":100}}"#,
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000060000,"usage":{"inputOther":50}}"#,
        ]);
        wire(&sub_dir, &[
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000120000,"usage":{"inputOther":15}}"#,
        ]);
        import_incremental_into(&sessions_root, &db_path).expect("多文件导入失败");
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        assert_eq!(usage_count(&conn), (3, 100 + 50 + 15), "两个 agent 文件应各计各的");

        // 第 2 步：main 重写为更短的 1 条且值更大（120 < 两行字节数 → reparse
        // 从 0 重计）：seq=1 撞旧 seq=1，120>100 覆盖；旧 seq=2 行按设计残留
        wire(&main_dir, &[
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000000000,"usage":{"inputOther":120}}"#,
        ]);
        import_incremental_into(&sessions_root, &db_path).expect("reparse 导入失败");
        assert_eq!(
            usage_count(&conn),
            (3, 120 + 50 + 15),
            "reparse 不得重复计数，大值应覆盖同键小值"
        );

        // 第 3 步：main 首行保持 120 不变、追加第二行 90（文件变长 → 续读）：
        // 序号恢复取该文件全部键（含上一步残留的 seq=2）的最大值 2，新行记为
        // seq=3，不与任何已有键冲突；残留的 50 按设计保留
        wire(&main_dir, &[
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000000000,"usage":{"inputOther":120}}"#,
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000060000,"usage":{"inputOther":90}}"#,
        ]);
        import_incremental_into(&sessions_root, &db_path).expect("续读导入失败");
        assert_eq!(
            usage_count(&conn),
            (4, 120 + 50 + 90 + 15),
            "续读序号应从该文件已入库最大序号（含残留）恢复"
        );

        // 第 4 步：main 再重写为更短的 1 条小值（80 < 120，触发 reparse 重放）：
        // 同键小值不得覆盖已入库大值
        wire(&main_dir, &[
            r#"{"type":"usage.record","model":"kimi-k2","time":1756000000000,"usage":{"inputOther":80}}"#,
        ]);
        import_incremental_into(&sessions_root, &db_path).expect("小值重放导入失败");
        assert_eq!(
            usage_count(&conn),
            (4, 120 + 50 + 90 + 15),
            "小值重放不得覆盖已入库大值"
        );
        // 全程样例无 llm.request 前置 → duration 全 NULL（每文件独立配对，
        // 不跨文件猜测、不用其他值兜底）
        let non_null: i64 = conn
            .query_row(
                "SELECT COUNT(*) FROM model_usage WHERE duration_ms IS NOT NULL",
                [],
                |row| row.get(0),
            )
            .unwrap();
        assert_eq!(non_null, 0, "无 llm.request 前置时 duration 应全为 NULL");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 耗时配对口径与合理性过滤（duration = usage.record.time - 最近一条
    /// llm.request.time）：正常配对 / 无前置请求 / 差值 <100ms / 差值 >600s /
    /// 连续多条 llm.request（重试）取最近一条。
    #[test]
    fn duration_pairing_and_bounds() {
        let tmp = std::env::temp_dir().join(format!("zbar-kimi-pair-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        let sessions_root = tmp.join("sessions");
        let db_path = tmp.join("kimi-pair.sqlite");
        let wire_dir = sessions_root.join("sess-p").join("agents").join("ag-1");
        std::fs::create_dir_all(&wire_dir).expect("创建样例目录失败");
        std::fs::write(
            wire_dir.join("wire.jsonl"),
            concat!(
                // 无前置 llm.request → NULL
                r#"{"type":"usage.record","model":"kimi-k2","time":500,"usage":{"inputOther":10}}"#,
                "\n",
                // 正常配对：65000 - 1000 = 64000
                r#"{"type":"llm.request","turnStep":"0.1","time":1000}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2","time":65000,"usage":{"inputOther":11}}"#,
                "\n",
                // 差值 50ms（<100）→ NULL
                r#"{"type":"llm.request","turnStep":"0.2","time":70000}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2","time":70050,"usage":{"inputOther":12}}"#,
                "\n",
                // 差值 700000ms（>600s）→ NULL
                r#"{"type":"llm.request","turnStep":"0.3","time":80000}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2","time":780000,"usage":{"inputOther":13}}"#,
                "\n",
                // 连续两条 llm.request（重试）→ 取最近一条：799000 - 795000 = 4000
                r#"{"type":"llm.request","turnStep":"0.4","time":790000}"#,
                "\n",
                r#"{"type":"llm.request","turnStep":"0.4","time":795000}"#,
                "\n",
                r#"{"type":"usage.record","model":"kimi-k2","time":799000,"usage":{"inputOther":14}}"#,
                "\n",
            ),
        )
        .expect("写入样例文件失败");

        import_incremental_into(&sessions_root, &db_path).expect("配对样例导入失败");
        let conn = Connection::open(&db_path).expect("打开临时库失败");
        assert_eq!(usage_count(&conn).0, 5, "配对样例应有 5 条有效记录");
        assert_eq!(
            durations(&conn),
            vec![None, Some(64_000), None, None, Some(4_000)],
            "耗时配对口径异常（边界过滤或取最近一条规则失效）"
        );

        let _ = std::fs::remove_dir_all(&tmp);
    }

    /// 旧库迁移：升级前创建的库（无 duration_ms 列）在 open 时自动 ALTER
    /// 补列，既有数据保留，新列可写（NULL 允许），重复 open 幂等。
    #[test]
    fn legacy_db_migration_adds_duration_column() {
        let tmp = std::env::temp_dir().join(format!("zbar-kimi-mig-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let db_path = tmp.join("kimi-legacy.sqlite");
        {
            // 旧版 DDL（无 duration_ms 列）建库并造 1 条数据
            let conn = Connection::open(&db_path).expect("建旧库失败");
            conn.execute_batch(
                "CREATE TABLE model_usage (
                    id INTEGER PRIMARY KEY AUTOINCREMENT,
                    session_id TEXT NOT NULL,
                    dedupe_key TEXT NOT NULL,
                    started_at INTEGER NOT NULL,
                    model_id TEXT NOT NULL DEFAULT '',
                    provider_id TEXT NOT NULL DEFAULT 'kimi',
                    input_tokens INTEGER NOT NULL DEFAULT 0,
                    output_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
                    cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
                    reasoning_tokens INTEGER NOT NULL DEFAULT 0,
                    computed_total_tokens INTEGER NOT NULL DEFAULT 0,
                    updated_at INTEGER NOT NULL DEFAULT 0,
                    UNIQUE(dedupe_key)
                 );
                 CREATE TABLE file_progress (
                    path   TEXT    PRIMARY KEY,
                    offset INTEGER NOT NULL,
                    size   INTEGER NOT NULL
                 );",
            )
            .expect("旧版建表失败");
            conn.execute(
                "INSERT INTO model_usage
                    (session_id, dedupe_key, started_at, model_id, computed_total_tokens)
                 VALUES ('s', 's|f|1', 1756000000000, 'kimi-k2', 100)",
                [],
            )
            .expect("写入旧数据失败");
            assert!(
                !db::has_column(&conn, "model_usage", "duration_ms"),
                "前置条件：旧库应无 duration_ms 列"
            );
        }
        // 重新 open：自动补列，旧数据保留
        let conn = open_kimi_db_at(&db_path).expect("迁移 open 失败");
        assert!(
            db::has_column(&conn, "model_usage", "duration_ms"),
            "迁移后应存在 duration_ms 列"
        );
        let (count, total): (i64, i64) = conn
            .query_row(
                "SELECT COUNT(*), COALESCE(SUM(computed_total_tokens),0) FROM model_usage",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .expect("迁移后计数失败");
        assert_eq!((count, total), (1, 100), "迁移不得丢既有数据");
        // 旧行 duration 自然为 NULL，且新列可写可读
        assert_eq!(durations(&conn), vec![None], "旧行 duration 应为 NULL");
        conn.execute(
            "UPDATE model_usage SET duration_ms = 64000 WHERE dedupe_key = 's|f|1'",
            [],
        )
        .expect("补列后写入失败");
        assert_eq!(durations(&conn), vec![Some(64_000)], "补列后读取失败");
        // 重复 open 幂等：列已存在不重复 ALTER、不报错
        drop(conn);
        open_kimi_db_at(&db_path).expect("二次 open（幂等迁移）失败");

        let _ = std::fs::remove_dir_all(&tmp);
    }
}

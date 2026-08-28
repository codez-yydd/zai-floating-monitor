//! 项目/会话浏览器后端。
//!
//! 数据来源与项目键（project_key）口径：
//! - Codex：会话文件首行 session_meta 事件的 payload.cwd → session_meta 表；
//! - Claude：会话 jsonl 行内顶层 cwd 字段 → model_usage 的 cwd/project_key 列；
//! - Kimi：wire.jsonl 头部 environmentDisclosure.cwd 行 → session_meta 表；
//! - ZCode：db.sqlite 的 model_usage 无 cwd 列，但主库 session_id 可关联
//!   zcode_sessions 派生库（~/.zbar/zcode_sessions.sqlite，解析主库 session
//!   表 directory 列 + agents/**/metadata.json 得 session_id → cwd 映射）。
//!   主路径：主库只读连接 ATTACH 派生库补项目归属，用量行仍读主库
//!   model_usage 本体（总量与主面板严格一致，永不相加）；派生库无映射/
//!   查询失败时回退旧逻辑（整体聚合进 __unknown__，不丢总量）。
//!
//! project_key = normalize_cwd(cwd)：字符串归一化（不做 fs::canonicalize，
//! 源目录可能已被删除），macOS/Windows 文件系统大小写不敏感故折叠小写，
//! 使 /Users/a/Proj 与 /users/a/proj 聚合到同一项目。

use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::{Mutex, OnceLock};

use crate::pricing::{load_pricing, ModelPrice};
use crate::{claude, codex, db, kimi, zcode_sessions, Billable, cost_for};

/// 未知项目的聚合键（无 cwd / 无项目维度的用量全部归入）
pub const UNKNOWN_PROJECT: &str = "__unknown__";

// ===== cwd 归一化 =====

/// 把原始 cwd 归一化为项目聚合键。按序执行：
/// 1. trim，空串返回 None；
/// 2. 展开 `~` / `$HOME` 前缀（home 目录缺失时返回 None）；
/// 3. `\` → `/`（Windows 路径分隔符）；
/// 4. macOS 已知前缀折叠：`/private/var` → `/var`、`/private/tmp` → `/tmp`
///    （只做字符串前缀映射，不做 fs::canonicalize——源目录可能已被删除）；
/// 5. 去尾部 `/`（保留根路径 `/`）；
/// 6. macOS/Windows 平台转小写聚合（目标平台 cfg 判断；文件系统大小写
///    不敏感），Linux 保留原样。
pub fn normalize_cwd(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return None;
    }
    // 展开 ~ / $HOME 前缀（home 定位失败视为无法归一化）
    let home = dirs::home_dir().map(|h| h.to_string_lossy().to_string());
    let expanded = if trimmed == "~" || trimmed == "$HOME" {
        home?
    } else if let Some(rest) = trimmed.strip_prefix("~/") {
        format!("{}/{}", home?, rest)
    } else if let Some(rest) = trimmed.strip_prefix("$HOME/") {
        format!("{}/{}", home?, rest)
    } else {
        trimmed.to_string()
    };

    // Windows 反斜杠统一为正斜杠
    let unified = expanded.replace('\\', "/");

    // macOS /private 前缀折叠（严格匹配到段边界，防 /private/varnish 误折叠）
    let fold = |from: &str, to: &str| -> Option<String> {
        if unified == from {
            return Some(to.to_string());
        }
        unified
            .strip_prefix(from)
            .filter(|rest| rest.starts_with('/'))
            .map(|rest| format!("{to}{rest}"))
    };
    let folded = fold("/private/var", "/var")
        .or_else(|| fold("/private/tmp", "/tmp"))
        .unwrap_or(unified);

    // 去尾部斜杠（保留根路径）
    let stripped = folded.trim_end_matches('/');
    let no_trailing = if stripped.is_empty() {
        "/".to_string()
    } else {
        stripped.to_string()
    };

    // macOS/Windows 大小写折叠（文件系统大小写不敏感，按小写聚合）
    let key;
    #[cfg(any(target_os = "macos", target_os = "windows"))]
    {
        key = no_trailing.to_lowercase();
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        key = no_trailing;
    }
    Some(key)
}

// ===== 数据结构（跨源公共行 + 命令返回）=====

/// 各源项目聚合查询返回的模型级行（codex/claude/kimi/zcode 共用）。
/// 花费由调用方结合价格表用 cost_for 计算（与 get_trend 同款口径）。
#[derive(Debug, Clone)]
pub(crate) struct ProjectModelRow {
    /// 归一化项目键；无 cwd 的会话为 UNKNOWN_PROJECT（SQL 侧 COALESCE）
    pub project_key: String,
    pub model_id: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
}

impl Billable for ProjectModelRow {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

/// 各源会话明细查询返回的「会话 × 模型」聚合行。
/// total token 不单独返回：SessionSummary 按四类 token 分列（契约固定），
/// 前端需要总量时自行求和。
#[derive(Debug, Clone)]
pub(crate) struct ProjectSessionModelRow {
    pub session_id: String,
    pub model_id: String,
    pub first_at: i64,
    pub last_at: i64,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    /// 可信速度行的 tps 总和与行数（口径同 db::speed_agg_columns 的噪声过滤）。
    /// 按「会话 × 模型」分组输出 SUM/COUNT，会话级 = 各分组 SUM/COUNT 分别
    /// 累加后相除（等价于面板公式限定会话范围，避免二次平均偏差）；
    /// 无耗时来源（codex）为 None/0。
    pub tps_sum: Option<f64>,
    pub tps_count: i64,
    /// 有效 TTFT 行的总和与行数（仅 zcode 主库有数据）
    pub ttft_sum: Option<f64>,
    pub ttft_count: i64,
}

impl Billable for ProjectSessionModelRow {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

/// get_projects 返回的单个项目汇总
#[derive(Debug, Clone, Serialize)]
pub struct ProjectSummary {
    /// 归一化键，未知项目为 "__unknown__"
    pub key: String,
    /// 原始形态路径（保留大小写），未知为 None
    pub display_path: Option<String>,
    pub is_unknown: bool,
    pub total_tokens: u64,
    pub requests: u64,
    pub cost_usd: f64,
    pub sessions: u32,
    pub by_agent: Vec<AgentBreakdown>,
}

/// 项目内按 Agent 来源的用量拆分
#[derive(Debug, Clone, Serialize)]
pub struct AgentBreakdown {
    /// "codex" | "claude" | "kimi" | "zcode"
    pub source: String,
    pub tokens: u64,
    pub requests: u64,
    pub cost_usd: f64,
    /// 无法统计会话数的源填 0（如 zcode 无 session 列时）
    pub sessions: u32,
}

/// get_project_sessions 返回的分页结构
#[derive(Debug, Clone, Serialize)]
pub struct SessionsPage {
    pub total: u32,
    pub items: Vec<SessionSummary>,
}

/// 单个会话的聚合摘要
#[derive(Debug, Clone, Serialize)]
pub struct SessionSummary {
    pub session_id: String,
    /// 会话所属源（"codex" | "claude" | "kimi" | "zcode"）
    pub source: String,
    /// 首末请求时间（毫秒）
    pub first_at: i64,
    pub last_at: i64,
    /// last_at - first_at（跨请求的墙钟跨度）
    pub wall_duration_ms: i64,
    /// 该会话用过的模型（去重）
    pub models: Vec<String>,
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub cache_read_tokens: u64,
    pub cache_write_tokens: u64,
    pub requests: u64,
    pub cost_usd: f64,
    /// 会话级平均输出速度（tok/s，口径与主面板一致；源无耗时数据为 None）
    pub speed_tps: Option<f64>,
    /// 会话级平均首字延迟（ms，仅 zcode 主库有 TTFT 数据；无则为 None）
    pub ttft_ms: Option<f64>,
}

// ===== 聚合辅助 =====

/// 单个项目内按来源累计的用量
#[derive(Default)]
struct AgentAcc {
    tokens: u64,
    requests: u64,
    cost_usd: f64,
    sessions: u32,
}

/// 单个项目的累计结构
#[derive(Default)]
struct ProjectAcc {
    display_path: Option<String>,
    by_agent: BTreeMap<String, AgentAcc>,
}

/// 把一条模型级行累加进项目聚合（花费按价格表计算，缺价模型不计入 cost）
fn accumulate(
    projects: &mut HashMap<String, ProjectAcc>,
    source: &str,
    row: &ProjectModelRow,
    usd: &BTreeMap<String, ModelPrice>,
) {
    let cost = cost_for(row, usd);
    let acc = projects.entry(row.project_key.clone()).or_default();
    let agent = acc.by_agent.entry(source.to_string()).or_default();
    agent.tokens += row.total_tokens.max(0) as u64;
    agent.requests += row.requests.max(0) as u64;
    agent.cost_usd += cost;
}

/// 项目浏览器查询前触发四源存量回填（各回填自带 30 秒节流 + 每批限量，
/// 未安装对应 CLI 时静默失败）。查询入口统一走这里，前端无需关心。
fn backfill_projects_dimensions() {
    let _ = codex::backfill_session_meta();
    let _ = claude::backfill_project_keys();
    let _ = kimi::backfill_session_meta();
    // zcode 派生库导入（rollout 镜像 + session 项目维度映射），失败静默：
    // 派生库无映射时 zcode 走旧回退路径
    let _ = zcode_sessions::import_incremental();
}

// ===== ZCode 派生库路径（M5：主库行 + ATTACH 派生库映射）=====

/// 打开主库只读连接并显式只读 ATTACH 派生库（别名 zs）。
/// 派生库文件必须已存在（导入器创建）；ATTACH 后 SQL 可直接
/// LEFT JOIN zs.session_meta 补项目归属。
/// 只读保证来自 mode=ro URI 而非主连接：ATTACH 打开被附加库的读写模式
/// 不能靠「主连接只读」隐式保证（未指定 mode 时的行为随 SQLite 版本/
/// 构建变化，本机 bundled 实测会沿用只读，但不可依赖），必须显式
/// file:...?mode=ro 才能把派生库稳定锁成只读（写语句将被拒），
/// 与「派生库绝不写入」的口径铁律对齐。
fn open_main_with_derived() -> Result<rusqlite::Connection, String> {
    let conn = zcode_sessions::open_main_db_readonly_uri()?;
    let derived = zcode_sessions::derived_db_path()?;
    if !derived.exists() {
        return Err(format!("ZCode 会话派生库尚未生成: {}", derived.display()));
    }
    let uri = zcode_sessions::derived_db_attach_uri()?;
    conn.execute("ATTACH DATABASE ?1 AS zs", rusqlite::params![uri])
        .map_err(|e| format!("ATTACH ZCode 会话派生库失败: {e}"))?;
    Ok(conn)
}

/// zcode 主路径：主库 model_usage（用量唯一来源）+ 派生库 session_meta
/// 项目归属。三段查询与 codex 同构：模型级行 GROUP BY、会话数
/// COUNT(DISTINCT session_id)、展示路径。无 cwd 映射的行归 __unknown__。
fn accumulate_zcode_with_derived(
    projects: &mut HashMap<String, ProjectAcc>,
    from_ms: i64,
    to_ms: i64,
    usd: &BTreeMap<String, ModelPrice>,
) -> Result<(), String> {
    let conn = open_main_with_derived()?;
    // 主库无 session_id 列（老版本 ZCode）时无法关联派生库，回退旧路径
    if !db::has_column(&conn, "model_usage", "session_id") {
        return Err("主库 model_usage 无 session_id 列".into());
    }

    // 模型级行（COALESCE 哨兵：无映射进 unknown）
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(sm.project_key, ?1),
                    mu.model_id,
                    COUNT(*),
                    COALESCE(SUM(mu.input_tokens),0),
                    COALESCE(SUM(mu.output_tokens),0),
                    COALESCE(SUM(mu.cache_read_input_tokens),0),
                    COALESCE(SUM(mu.computed_total_tokens),0)
             FROM model_usage mu
             LEFT JOIN zs.session_meta sm ON sm.session_id = mu.session_id
             WHERE mu.started_at >= ?2 AND mu.started_at < ?3
             GROUP BY 1, 2",
        )
        .map_err(|e| format!("准备 zcode 派生库项目聚合查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![UNKNOWN_PROJECT, from_ms, to_ms], |row| {
            Ok(ProjectModelRow {
                project_key: row.get(0)?,
                model_id: row.get(1)?,
                requests: row.get(2)?,
                input_tokens: row.get(3)?,
                output_tokens: row.get(4)?,
                cache_read_tokens: row.get(5)?,
                total_tokens: row.get(6)?,
            })
        })
        .map_err(|e| format!("读取 zcode 派生库项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 派生库项目聚合失败: {e}"))?;
    for row in &rows {
        accumulate(projects, "zcode", row, usd);
    }

    // 会话数（项目粒度）
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(sm.project_key, ?1), COUNT(DISTINCT mu.session_id)
             FROM model_usage mu
             LEFT JOIN zs.session_meta sm ON sm.session_id = mu.session_id
             WHERE mu.started_at >= ?2 AND mu.started_at < ?3
             GROUP BY 1",
        )
        .map_err(|e| format!("准备 zcode 派生库会话统计查询失败: {e}"))?;
    let counts = stmt
        .query_map(rusqlite::params![UNKNOWN_PROJECT, from_ms, to_ms], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })
        .map_err(|e| format!("读取 zcode 派生库会话统计失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 派生库会话统计失败: {e}"))?;
    for (key, count) in counts {
        let acc = projects.entry(key).or_default();
        let agent = acc.by_agent.entry("zcode".to_string()).or_default();
        agent.sessions += count.max(0) as u32;
    }

    // 展示路径（派生库中的原始形态 cwd，保留大小写）
    let mut stmt = conn
        .prepare(
            "SELECT project_key, MIN(cwd)
             FROM zs.session_meta
             WHERE project_key IS NOT NULL AND cwd IS NOT NULL AND cwd != ''
             GROUP BY project_key",
        )
        .map_err(|e| format!("准备 zcode 派生库路径查询失败: {e}"))?;
    let paths = stmt
        .query_map([], |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)))
        .map_err(|e| format!("读取 zcode 派生库路径失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 派生库路径失败: {e}"))?;
    merge_display_paths(projects, paths);
    Ok(())
}

/// zcode 会话明细主路径：派生库映射 + 主库行（SQL 侧分页，与 codex
/// 同构）。project_key 传 UNKNOWN_PROJECT 时匹配无映射的会话。
fn zcode_project_sessions_with_derived(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
    let conn = open_main_with_derived()?;
    if !db::has_column(&conn, "model_usage", "session_id") {
        return Err("主库 model_usage 无 session_id 列".into());
    }
    let total: i64 = conn
        .query_row(
            "SELECT COUNT(*) FROM (
                SELECT DISTINCT mu.session_id
                FROM model_usage mu
                LEFT JOIN zs.session_meta sm ON sm.session_id = mu.session_id
                WHERE mu.started_at >= ?1 AND mu.started_at < ?2
                  AND COALESCE(sm.project_key, ?3) = ?4
            )",
            rusqlite::params![from_ms, to_ms, UNKNOWN_PROJECT, project_key],
            |row| row.get(0),
        )
        .map_err(|e| format!("查询 zcode 派生库项目会话总数失败: {e}"))?;
    // 速度/TTFT 聚合列（口径与主面板 db::speed_agg_columns 一致，传 SUM/COUNT
    // 供会话级合并；按列有无动态降级，同 db::query_stats 的探测习惯）。
    // 表达式引用的 output_tokens/duration_ms/time_to_first_token_ms 仅主库
    // model_usage（别名 mu）持有，JOIN 的 zs.session_meta 无同名列，无歧义。
    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        db::has_column(&conn, "model_usage", "time_to_first_token_ms"),
    );
    let mut stmt = conn
        .prepare(
            &format!(
                "SELECT mu.session_id, mu.model_id,
                        MIN(mu.started_at), MAX(mu.started_at), COUNT(*),
                        COALESCE(SUM(mu.input_tokens),0),
                        COALESCE(SUM(mu.output_tokens),0),
                        COALESCE(SUM(mu.cache_read_input_tokens),0),
                        COALESCE(SUM(mu.cache_creation_input_tokens),0)
                        {speed}
                 FROM model_usage mu
                 LEFT JOIN zs.session_meta sm ON sm.session_id = mu.session_id
                 WHERE mu.started_at >= ?1 AND mu.started_at < ?2
                   AND COALESCE(sm.project_key, ?3) = ?4
                   AND mu.session_id IN (
                       SELECT session_id FROM (
                           SELECT mu2.session_id AS session_id
                           FROM model_usage mu2
                           LEFT JOIN zs.session_meta sm2 ON sm2.session_id = mu2.session_id
                           WHERE mu2.started_at >= ?1 AND mu2.started_at < ?2
                             AND COALESCE(sm2.project_key, ?3) = ?4
                           GROUP BY mu2.session_id
                           ORDER BY MAX(mu2.started_at) DESC
                           LIMIT ?5 OFFSET ?6
                       )
                   )
                 GROUP BY mu.session_id, mu.model_id"
            ),
        )
        .map_err(|e| format!("准备 zcode 派生库项目会话查询失败: {e}"))?;
    let rows = stmt
        .query_map(
            rusqlite::params![
                from_ms,
                to_ms,
                UNKNOWN_PROJECT,
                project_key,
                limit as i64,
                offset as i64
            ],
            |row| {
                Ok(ProjectSessionModelRow {
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
        .map_err(|e| format!("读取 zcode 派生库项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 派生库项目会话失败: {e}"))?;
    Ok((total.max(0) as u32, rows))
}

// ===== ZCode 列探测（运行时能力探测 + 进程内缓存）=====

/// zcode 库项目维度可用列的探测结果。
#[derive(Debug, Clone, Default)]
struct ZcodeProjectColumns {
    /// cwd/项目类列名（能推导项目键；None = 无此类列，整体归未知项目）
    cwd_col: Option<String>,
    /// 会话 id 类列名（统计会话数；None = 无法统计，sessions 恒 0）
    session_col: Option<String>,
}

/// 探测结果进程内缓存：None = 尚无成功探测结果。只缓存探测成功的结果
/// （含「无 cwd/会话列」的降级形态）；库不可用等失败不缓存，下次查询重试
/// （探测仅 PRAGMA 查询，开销极小），避免 WAL 锁占用等瞬时错误导致
/// zcode 在整个进程生命周期内静默消失。
static ZCODE_PROJECT_PROBE: OnceLock<Mutex<Option<ZcodeProjectColumns>>> = OnceLock::new();

/// zcode 库可用的项目维度列（候选名探测 + 成功结果缓存）。
/// 候选：cwd / project / project_id / workspace（项目键）与
/// session_id / sessionid（会话数）。当前版本 zcode 库无 cwd 类列 →
/// 用量整体聚合进 __unknown__（sessions=0，会话列表不展示 zcode 明细）。
/// 库不可用时返回 None：打印日志但不缓存，下次查询重试。
fn probe_zcode_project_columns() -> Option<ZcodeProjectColumns> {
    let cache = ZCODE_PROJECT_PROBE.get_or_init(|| Mutex::new(None));
    {
        let guard = cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(cached) = guard.as_ref() {
            return Some(cached.clone());
        }
    }
    match probe_zcode_project_columns_uncached() {
        Ok(probed) => {
            *cache.lock().unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(probed.clone());
            Some(probed)
        }
        Err(e) => {
            eprintln!("[projects] zcode 库探测失败（本次查询跳过 zcode，下次重试）: {e}");
            None
        }
    }
}

/// 真实探测（不查缓存）：库不可用返回 Err，可用则返回列探测结果
/// （cwd/会话类列均不存在时为全 None 的默认结构，属成功形态）。
fn probe_zcode_project_columns_uncached() -> Result<ZcodeProjectColumns, String> {
    let conn = db::open_db()?;
    let cwd_col = ["cwd", "project", "project_id", "workspace"]
        .iter()
        .find(|c| db::has_column(&conn, "model_usage", c))
        .map(|c| c.to_string());
    let session_col = ["session_id", "sessionid"]
        .iter()
        .find(|c| db::has_column(&conn, "model_usage", c))
        .map(|c| c.to_string());
    Ok(ZcodeProjectColumns {
        cwd_col,
        session_col,
    })
}

/// zcode 的项目聚合。优先走派生库映射路径（主库行 + ATTACH zs.session_meta，
/// 总量与主面板严格一致）；派生库无映射或查询失败时回退列探测路径
/// （探测有 cwd 类列 → 直读；无 → 全部聚合进未知项目，sessions=0）。
/// 任一路径失败都打印日志跳过（不影响其他源，也不向前端报错）。
fn accumulate_zcode(
    projects: &mut HashMap<String, ProjectAcc>,
    from_ms: i64,
    to_ms: i64,
    usd: &BTreeMap<String, ModelPrice>,
) {
    // 主路径：派生库已有项目映射（导入器写入过非 NULL project_key）
    if zcode_sessions::has_project_mapping() {
        match accumulate_zcode_with_derived(projects, from_ms, to_ms, usd) {
            Ok(()) => return,
            Err(e) => {
                eprintln!("[projects] zcode 派生库聚合失败（本次回退旧路径）: {e}");
            }
        }
    }

    // 回退路径：探测失败（库不可用）已在 probe 内打印日志
    let Some(columns) = probe_zcode_project_columns() else {
        return;
    };
    let Some(cwd_col) = columns.cwd_col.as_deref() else {
        // 无项目维度列：全部用量聚合进 __unknown__（sessions 恒 0）
        let rows = match zcode_model_rows_without_project(from_ms, to_ms) {
            Ok(rows) => rows,
            Err(e) => {
                eprintln!("[projects] zcode 项目用量查询失败（本次跳过 zcode）: {e}");
                return;
            }
        };
        for mut row in rows {
            row.project_key = UNKNOWN_PROJECT.to_string();
            accumulate(projects, "zcode", &row, usd);
        }
        return;
    };

    // 有 cwd 类列：直读原始值，Rust 侧归一化聚合（SQL 无法做大小写折叠）
    let conn = match db::open_db() {
        Ok(conn) => conn,
        Err(e) => {
            eprintln!("[projects] zcode 库打开失败（本次跳过会话/路径统计）: {e}");
            return;
        }
    };
    let sql = format!(
        "SELECT \"{cwd_col}\", model_id,
                COUNT(*),
                COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(computed_total_tokens),0)
         FROM model_usage
         WHERE started_at >= ?1 AND started_at < ?2
         GROUP BY \"{cwd_col}\", model_id"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return;
    };
    let rows = match stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                ProjectModelRow {
                    project_key: String::new(),
                    model_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                },
            ))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    {
        Ok(rows) => rows,
        Err(e) => {
            eprintln!("[projects] zcode 项目用量直读失败（本次跳过）: {e}");
            return;
        }
    };
    for (raw, mut row) in rows {
        row.project_key = raw
            .as_deref()
            .and_then(normalize_cwd)
            .unwrap_or_else(|| UNKNOWN_PROJECT.to_string());
        accumulate(projects, "zcode", &row, usd);
    }

    // 会话数与展示路径（有会话列才可统计）
    let Some(session_col) = columns.session_col.as_deref() else {
        return;
    };
    let sql = format!(
        "SELECT \"{cwd_col}\", COUNT(DISTINCT \"{session_col}\")
         FROM model_usage
         WHERE started_at >= ?1 AND started_at < ?2
         GROUP BY \"{cwd_col}\""
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return;
    };
    let counts = match stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok((row.get::<_, Option<String>>(0)?, row.get::<_, i64>(1)?))
        })
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    {
        Ok(counts) => counts,
        Err(e) => {
            eprintln!("[projects] zcode 项目会话数查询失败（本次跳过）: {e}");
            return;
        }
    };
    for (raw, count) in counts {
        if let Some(key) = raw.as_deref().and_then(normalize_cwd) {
            let acc = projects.entry(key).or_default();
            let agent = acc.by_agent.entry("zcode".to_string()).or_default();
            agent.sessions += count.max(0) as u32;
        }
    }
    // 展示路径：取每个原始形态的代表（首个非空值）
    let sql = format!(
        "SELECT DISTINCT \"{cwd_col}\" FROM model_usage
         WHERE \"{cwd_col}\" IS NOT NULL AND \"{cwd_col}\" != ''"
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return;
    };
    let paths = match stmt
        .query_map([], |row| row.get::<_, String>(0))
        .and_then(|rows| rows.collect::<Result<Vec<_>, _>>())
    {
        Ok(paths) => paths,
        Err(e) => {
            eprintln!("[projects] zcode 项目展示路径查询失败（本次跳过）: {e}");
            return;
        }
    };
    merge_display_paths(projects, paths.into_iter().map(|raw| {
        let key = normalize_cwd(&raw).unwrap_or_else(|| UNKNOWN_PROJECT.to_string());
        (key, raw)
    }));
}

/// zcode 无项目维度时的全量模型聚合（行内 project_key 由调用方覆写为 unknown）
fn zcode_model_rows_without_project(from_ms: i64, to_ms: i64) -> Result<Vec<ProjectModelRow>, String> {
    let conn = db::open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT model_id,
                    COUNT(*),
                    COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                    COALESCE(SUM(cache_read_input_tokens),0),
                    COALESCE(SUM(computed_total_tokens),0)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2
             GROUP BY model_id",
        )
        .map_err(|e| format!("准备 zcode 项目聚合查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok(ProjectModelRow {
                project_key: String::new(),
                model_id: row.get(0)?,
                requests: row.get(1)?,
                input_tokens: row.get(2)?,
                output_tokens: row.get(3)?,
                cache_read_tokens: row.get(4)?,
                total_tokens: row.get(5)?,
            })
        })
        .map_err(|e| format!("读取 zcode 项目聚合失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 项目聚合失败: {e}"))?;
    Ok(rows)
}

/// zcode 的会话明细。优先走派生库映射路径（SQL 侧分页，与 codex 同构）；
/// 无映射/查询失败时回退列探测路径（仅 cwd 类列 + 会话列都存在时支持，
/// 否则返回空页）。
fn zcode_project_sessions(
    project_key: &str,
    from_ms: i64,
    to_ms: i64,
    offset: u32,
    limit: u32,
) -> Result<(u32, Vec<ProjectSessionModelRow>), String> {
    // 主路径：派生库已有项目映射
    if zcode_sessions::has_project_mapping() {
        match zcode_project_sessions_with_derived(project_key, from_ms, to_ms, offset, limit) {
            Ok(page) => return Ok(page),
            Err(e) => {
                eprintln!("[projects] zcode 派生库会话查询失败（本次回退旧路径）: {e}");
            }
        }
    }

    let Some(columns) = probe_zcode_project_columns() else {
        return Ok((0, Vec::new()));
    };
    let (Some(cwd_col), Some(session_col)) = (columns.cwd_col.as_deref(), columns.session_col.as_deref())
    else {
        // 无项目/会话维度（当前版本 zcode 库的实际形态）：会话列表不展示
        return Ok((0, Vec::new()));
    };
    let conn = db::open_db()?;
    // 速度/TTFT 聚合列（口径同主面板，按列有无动态降级）
    let speed = db::session_speed_agg_columns(
        db::has_column(&conn, "model_usage", "duration_ms"),
        db::has_column(&conn, "model_usage", "time_to_first_token_ms"),
    );
    let sql = format!(
        "SELECT \"{session_col}\", \"{cwd_col}\", model_id,
                MIN(started_at), MAX(started_at), COUNT(*),
                COALESCE(SUM(input_tokens),0), COALESCE(SUM(output_tokens),0),
                COALESCE(SUM(cache_read_input_tokens),0),
                COALESCE(SUM(cache_creation_input_tokens),0)
                {speed}
         FROM model_usage
         WHERE started_at >= ?1 AND started_at < ?2
         GROUP BY \"{session_col}\", \"{cwd_col}\", model_id"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 zcode 项目会话查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok((
                row.get::<_, Option<String>>(1)?,
                ProjectSessionModelRow {
                    session_id: row.get(0)?,
                    model_id: row.get(2)?,
                    first_at: row.get(3)?,
                    last_at: row.get(4)?,
                    requests: row.get(5)?,
                    input_tokens: row.get(6)?,
                    output_tokens: row.get(7)?,
                    cache_read_tokens: row.get(8)?,
                    cache_write_tokens: row.get(9)?,
                    tps_sum: row.get(10)?,
                    tps_count: row.get(11)?,
                    ttft_sum: row.get(12)?,
                    ttft_count: row.get(13)?,
                },
            ))
        })
        .map_err(|e| format!("读取 zcode 项目会话失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 zcode 项目会话失败: {e}"))?;

    // 按归一化键过滤 + 组装（内存侧，仅直读场景走到）
    struct SessionBuild {
        rows: Vec<ProjectSessionModelRow>,
        first_at: i64,
        last_at: i64,
    }
    let mut by_session: BTreeMap<String, SessionBuild> = BTreeMap::new();
    for (raw_cwd, row) in rows {
        let key = raw_cwd
            .as_deref()
            .and_then(normalize_cwd)
            .unwrap_or_else(|| UNKNOWN_PROJECT.to_string());
        if key != project_key {
            continue;
        }
        let build = by_session
            .entry(row.session_id.clone())
            .or_insert_with(|| SessionBuild {
                rows: Vec::new(),
                first_at: i64::MAX,
                last_at: i64::MIN,
            });
        build.first_at = build.first_at.min(row.first_at);
        build.last_at = build.last_at.max(row.last_at);
        build.rows.push(row);
    }
    let total = by_session.len() as u32;
    let mut sessions: Vec<(i64, Vec<ProjectSessionModelRow>)> = by_session
        .into_iter()
        .map(|(_, build)| (build.last_at, build.rows))
        .collect();
    sessions.sort_by(|a, b| b.0.cmp(&a.0));
    let page = sessions
        .into_iter()
        .skip(offset as usize)
        .take(limit as usize)
        .flat_map(|(_, rows)| rows);
    Ok((total, page.collect()))
}

/// 合并展示路径（同键已有值不覆盖；未知项目键忽略）
fn merge_display_paths<I>(projects: &mut HashMap<String, ProjectAcc>, paths: I)
where
    I: IntoIterator<Item = (String, String)>,
{
    for (key, raw) in paths {
        if key == UNKNOWN_PROJECT {
            continue;
        }
        let acc = projects.entry(key).or_default();
        if acc.display_path.is_none() {
            acc.display_path = Some(raw);
        }
    }
}

// ===== 查询实现 =====

/// 项目列表查询：各源 SQL 侧 GROUP BY 聚合 → Rust 侧按 key 合并跨源分组；
/// 花费复用 cost_for 计费口径（缺价模型不计入 cost）。
fn query_projects(from_ms: i64, to_ms: i64) -> Result<Vec<ProjectSummary>, String> {
    backfill_projects_dimensions();
    let pricing = load_pricing().unwrap_or_default();
    let mut projects: HashMap<String, ProjectAcc> = HashMap::new();

    // codex / claude / kimi 三源（未安装/查询失败打印日志并降级为空，不互相阻断）
    let model_rows = [
        ("codex", codex::query_project_model_rows(from_ms, to_ms)),
        ("claude", claude::query_project_model_rows(from_ms, to_ms)),
        ("kimi", kimi::query_project_model_rows(from_ms, to_ms)),
    ];
    for (source, rows) in model_rows {
        match rows {
            Ok(rows) => {
                for row in &rows {
                    accumulate(&mut projects, source, row, &pricing.usd);
                }
            }
            Err(e) => eprintln!("[projects] {source} 项目用量查询失败（本次跳过该源）: {e}"),
        }
    }

    // 会话数（COUNT(DISTINCT session_id)，项目粒度）
    let session_counts = [
        ("codex", codex::query_project_session_counts(from_ms, to_ms)),
        ("claude", claude::query_project_session_counts(from_ms, to_ms)),
        ("kimi", kimi::query_project_session_counts(from_ms, to_ms)),
    ];
    for (source, counts) in session_counts {
        match counts {
            Ok(counts) => {
                for (key, count) in counts {
                    let acc = projects.entry(key).or_default();
                    let agent = acc.by_agent.entry(source.to_string()).or_default();
                    agent.sessions += count.max(0) as u32;
                }
            }
            Err(e) => eprintln!("[projects] {source} 项目会话数查询失败（本次跳过）: {e}"),
        }
    }

    // 展示路径（原始形态 cwd，保留大小写）
    let display_paths = [
        ("codex", codex::query_project_display_paths()),
        ("claude", claude::query_project_display_paths()),
        ("kimi", kimi::query_project_display_paths()),
    ];
    for (source, paths) in display_paths {
        match paths {
            Ok(paths) => merge_display_paths(&mut projects, paths),
            Err(e) => eprintln!("[projects] {source} 展示路径查询失败（本次跳过）: {e}"),
        }
    }

    // zcode（列探测：无 cwd 类列时整体聚合进未知项目，sessions=0）
    accumulate_zcode(&mut projects, from_ms, to_ms, &pricing.usd);

    // 组装输出（总量降序；同量时未知项目排最后）
    let mut out: Vec<ProjectSummary> = projects
        .into_iter()
        .map(|(key, acc)| {
            let is_unknown = key == UNKNOWN_PROJECT;
            let (total_tokens, requests, cost_usd) = acc
                .by_agent
                .values()
                .fold((0u64, 0u64, 0.0), |(t, r, c), a| {
                    (t + a.tokens, r + a.requests, c + a.cost_usd)
                });
            ProjectSummary {
                key: key.clone(),
                display_path: if is_unknown { None } else { acc.display_path },
                is_unknown,
                total_tokens,
                requests,
                cost_usd,
                sessions: acc.by_agent.values().map(|a| a.sessions).sum(),
                by_agent: acc
                    .by_agent
                    .into_iter()
                    .map(|(source, a)| AgentBreakdown {
                        source,
                        tokens: a.tokens,
                        requests: a.requests,
                        cost_usd: a.cost_usd,
                        sessions: a.sessions,
                    })
                    .collect(),
            }
        })
        .collect();
    out.sort_by(|a, b| {
        b.total_tokens
            .cmp(&a.total_tokens)
            .then(a.is_unknown.cmp(&b.is_unknown))
    });
    Ok(out)
}

/// 会话构建中间结构（按 (source, session_id) 聚合模型级行）
#[derive(Default)]
struct SessionBuild {
    first_at: i64,
    last_at: i64,
    models: BTreeSet<String>,
    input_tokens: u64,
    output_tokens: u64,
    cache_read_tokens: u64,
    cache_write_tokens: u64,
    requests: u64,
    cost_usd: f64,
    /// 可信速度行 tps 总和与行数（跨「会话 × 模型」分组累加 SUM/COUNT，
    /// 会话级平均 = 总和 / 行数，口径与主面板整体平均一致）
    tps_sum: Option<f64>,
    tps_count: i64,
    /// 有效 TTFT 行总和与行数（仅 zcode 有数据）
    ttft_sum: Option<f64>,
    ttft_count: i64,
}

/// 跨分组的 SUM 合并（None 视为该分组无可信样本，跳过）
fn sum_opt(a: Option<f64>, b: Option<f64>) -> Option<f64> {
    match (a, b) {
        (Some(x), Some(y)) => Some(x + y),
        (Some(x), None) => Some(x),
        (None, Some(y)) => Some(y),
        (None, None) => None,
    }
}

/// 项目会话分页查询。source 为 None 时跨源合并（各源取全部匹配行，
/// Rust 侧按最后活跃时间排序后统一分页）；指定单源时 SQL 侧 LIMIT/OFFSET
/// 分页。未知项目在 zcode 无会话维度时自然不返回 zcode 明细。
fn query_sessions(
    project_key: String,
    from_ms: i64,
    to_ms: i64,
    source: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<SessionsPage, String> {
    backfill_projects_dimensions();
    let pricing = load_pricing().unwrap_or_default();

    // 参与查询的源（非法源名返回空页，不报错）
    let sources: Vec<&str> = match source.as_deref() {
        Some(s) if !s.is_empty() => {
            if !["codex", "claude", "kimi", "zcode", "cursor"].contains(&s) {
                return Ok(SessionsPage {
                    total: 0,
                    items: Vec::new(),
                });
            }
            vec![s]
        }
        _ => vec!["codex", "claude", "kimi", "zcode"],
    };

    // 单源走 SQL 分页；多源各取全部匹配行（行数 = 会话 × 模型，量级可控）
    let single = sources.len() == 1;
    let per_offset = if single { offset } else { 0 };
    let per_limit = if single { limit } else { u32::MAX };

    let mut builders: HashMap<(String, String), SessionBuild> = HashMap::new();
    let mut total: u32 = 0;
    for src in sources {
        let result = match src {
            "codex" => codex::query_project_sessions(&project_key, from_ms, to_ms, per_offset, per_limit),
            "claude" => claude::query_project_sessions(&project_key, from_ms, to_ms, per_offset, per_limit),
            "kimi" => kimi::query_project_sessions(&project_key, from_ms, to_ms, per_offset, per_limit),
            "zcode" => zcode_project_sessions(&project_key, from_ms, to_ms, per_offset, per_limit),
            // cursor 无会话维度（纯内存聚合），会话列表不展示
            "cursor" => Ok((0, Vec::new())),
            _ => unreachable!("来源已在上文校验"),
        };
        // 未安装对应 CLI / 查询失败静默降级为空，不阻断其他源
        let (t, rows) = result.unwrap_or((0, Vec::new()));
        total += t;
        for row in rows {
            let build = builders
                .entry((src.to_string(), row.session_id.clone()))
                .or_insert_with(|| SessionBuild {
                    first_at: i64::MAX,
                    last_at: i64::MIN,
                    ..Default::default()
                });
            build.first_at = build.first_at.min(row.first_at);
            build.last_at = build.last_at.max(row.last_at);
            build.models.insert(row.model_id.clone());
            build.input_tokens += row.input_tokens.max(0) as u64;
            build.output_tokens += row.output_tokens.max(0) as u64;
            build.cache_read_tokens += row.cache_read_tokens.max(0) as u64;
            build.cache_write_tokens += row.cache_write_tokens.max(0) as u64;
            build.requests += row.requests.max(0) as u64;
            build.cost_usd += cost_for(&row, &pricing.usd);
            build.tps_sum = sum_opt(build.tps_sum, row.tps_sum);
            build.tps_count += row.tps_count.max(0);
            build.ttft_sum = sum_opt(build.ttft_sum, row.ttft_sum);
            build.ttft_count += row.ttft_count.max(0);
        }
    }

    let mut items: Vec<SessionSummary> = builders
        .into_iter()
        .map(|((source, session_id), build)| SessionSummary {
            session_id,
            source,
            wall_duration_ms: (build.last_at - build.first_at).max(0),
            first_at: build.first_at,
            last_at: build.last_at,
            models: build.models.into_iter().collect(),
            input_tokens: build.input_tokens,
            output_tokens: build.output_tokens,
            cache_read_tokens: build.cache_read_tokens,
            cache_write_tokens: build.cache_write_tokens,
            requests: build.requests,
            cost_usd: build.cost_usd,
            // 会话级平均 = 可信样本总和 / 样本数（无可信样本时为 None）
            speed_tps: match build.tps_sum {
                Some(sum) if build.tps_count > 0 => Some(sum / build.tps_count as f64),
                _ => None,
            },
            ttft_ms: match build.ttft_sum {
                Some(sum) if build.ttft_count > 0 => Some(sum / build.ttft_count as f64),
                _ => None,
            },
        })
        .collect();
    items.sort_by(|a, b| b.last_at.cmp(&a.last_at).then(a.session_id.cmp(&b.session_id)));

    // 多源合并时在 Rust 侧统一分页
    let items = if single {
        items
    } else {
        items
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .collect()
    };
    Ok(SessionsPage { total, items })
}

// ===== Tauri 命令（前端契约，签名不可改）=====

/// get_projects：返回时间范围内按项目聚合的用量（跨 codex/claude/kimi/
/// zcode 四源合并，cursor 无项目维度不在项目浏览器展示）。内部含 SQLite
/// 查询与三源存量回填（文件 IO），async + spawn_blocking 卸载到阻塞线程池，
/// 避免阻塞主线程。
#[tauri::command]
pub async fn get_projects(from_ms: i64, to_ms: i64) -> Result<Vec<ProjectSummary>, String> {
    tauri::async_runtime::spawn_blocking(move || query_projects(from_ms, to_ms))
        .await
        .map_err(|e| format!("项目查询任务失败: {e}"))?
}

/// get_project_sessions：分页返回指定项目的会话明细。
/// source 可选过滤（"codex" | "claude" | "kimi" | "zcode" | "cursor"）。
/// 查询入口先走各源现成的增量导入（含 5 秒节流）与存量回填。
#[tauri::command]
pub async fn get_project_sessions(
    project_key: String,
    from_ms: i64,
    to_ms: i64,
    source: Option<String>,
    offset: u32,
    limit: u32,
) -> Result<SessionsPage, String> {
    tauri::async_runtime::spawn_blocking(move || {
        query_sessions(project_key, from_ms, to_ms, source, offset, limit)
    })
    .await
    .map_err(|e| format!("项目会话查询任务失败: {e}"))?
}

#[cfg(test)]
mod tests {
    use super::*;

    /// cwd 归一化：空串/空白、~ 与 $HOME 展开、反斜杠、/private 前缀折叠、
    /// 尾部斜杠、根路径保留。大小写折叠按目标平台分别断言。
    #[test]
    fn normalize_cwd_variants() {
        assert_eq!(normalize_cwd(""), None);
        assert_eq!(normalize_cwd("   "), None);

        // 尾部斜杠去除 + 普通路径保留形态
        let plain = normalize_cwd("/Users/a/proj/").unwrap();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(plain, "/users/a/proj");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(plain, "/Users/a/proj");

        // Windows 反斜杠 → 正斜杠
        let win = normalize_cwd("C:\\Work\\Demo\\").unwrap();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(win, "c:/work/demo");
        #[cfg(not(any(target_os = "macos", target_os = "windows")))]
        assert_eq!(win, "C:/Work/Demo");

        // macOS /private 前缀折叠（段边界严格匹配）
        let folded = normalize_cwd("/private/var/folders/abc").unwrap();
        #[cfg(any(target_os = "macos", target_os = "windows"))]
        assert_eq!(folded, "/var/folders/abc");
        // /private/varnish 不是 /private/var 段，不得折叠
        let no_fold = normalize_cwd("/private/varnish/x").unwrap();
        assert!(no_fold.starts_with("/private/varnish"), "{no_fold}");
        // 单段前缀本身也折叠
        assert_eq!(
            normalize_cwd("/private/tmp").unwrap(),
            {
                #[cfg(any(target_os = "macos", target_os = "windows"))]
                { "/tmp".to_string() }
                #[cfg(not(any(target_os = "macos", target_os = "windows")))]
                { "/private/tmp".to_string() }
            }
        );

        // 根路径保留（去尾斜杠后为空 → 回填为 "/"）
        assert_eq!(normalize_cwd("/").unwrap(), "/");
        assert_eq!(normalize_cwd("//").unwrap(), "/");

        // ~ / $HOME 展开
        let home = dirs::home_dir().unwrap().to_string_lossy().to_string();
        let tilde = normalize_cwd("~/code/demo").unwrap();
        let expect_home = {
            #[cfg(any(target_os = "macos", target_os = "windows"))]
            { home.to_lowercase() }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            { home.clone() }
        };
        assert!(tilde.starts_with(&expect_home), "{tilde} vs {expect_home}");
        assert!(tilde.ends_with("code/demo"));
        let env_home = normalize_cwd("$HOME/code/x").unwrap();
        assert!(env_home.starts_with(&expect_home));
    }

    /// 探测缓存行为：重复调用结果稳定（进程内缓存命中，不重复开库）。
    /// 本机 zcode 库有 session_id 列但无 cwd 类列时呈"无项目维度"降级形态。
    #[test]
    fn zcode_probe_returns_stable_result() {
        let first = probe_zcode_project_columns();
        let second = probe_zcode_project_columns();
        match (&first, &second) {
            (Some(a), Some(b)) => assert_eq!(a.cwd_col, b.cwd_col),
            (None, None) => {}
            _ => panic!("探测结果不稳定: {first:?} vs {second:?}"),
        }
    }

    /// 回归诊断：项目浏览器「今日」范围必须包含 zcode 来源，且总量与主库
    /// 直查严格一致（M5 起派生库映射把 zcode 归入真实项目，总量仍读主库
    /// model_usage 本体，两侧读同一张表 → 差异恒 0，防止双计或丢量）。
    /// 防止 zcode 再次静默消失（历史缺陷：探测失败结果曾被永久缓存）。
    /// 注意：依赖本机 zcode 库当日有用量数据。
    #[test]
    fn query_projects_today_totals_match_zcode_main() {
        use chrono::Timelike;
        let now = chrono::Local::now();
        // 今日本地零点（毫秒，与库中 started_at 口径一致）；纯时间运算，避免夏令时边界
        let from_ms = now.timestamp_millis()
            - (now.time().num_seconds_from_midnight() as i64) * 1000
            - now.timestamp_subsec_millis() as i64;
        let to_ms = now.timestamp_millis() + 60_000;

        let projects = query_projects(from_ms, to_ms).expect("query_projects 不应失败");
        // 跨全部项目（含 __unknown__）汇总 zcode 来源用量
        let (mut zcode_tokens, mut zcode_requests) = (0u64, 0u64);
        let mut mapped_sessions = 0u32;
        for p in &projects {
            if let Some(a) = p.by_agent.iter().find(|a| a.source == "zcode") {
                zcode_tokens += a.tokens;
                zcode_requests += a.requests;
                if !p.is_unknown {
                    mapped_sessions += a.sessions;
                }
            }
        }
        if zcode_tokens == 0 && zcode_requests == 0 {
            eprintln!("诊断跳过：本机 zcode 今日无用量（query_projects 返回 {} 个项目）", projects.len());
            return;
        }

        // 主库直查（与主面板统计同源同口径）
        let main_rows = zcode_model_rows_without_project(from_ms, to_ms)
            .expect("主库直查不应失败");
        let (main_tokens, main_reqs): (u64, u64) = main_rows.iter().fold((0, 0), |(t, r), row| {
            (t + row.total_tokens.max(0) as u64, r + row.requests.max(0) as u64)
        });
        assert!(main_tokens > 0, "主库今日 tokens 应大于 0");
        assert_eq!(
            zcode_tokens, main_tokens,
            "项目浏览器 zcode 今日 tokens（{zcode_tokens}）必须与主库直查（{main_tokens}）一致"
        );
        assert_eq!(
            zcode_requests, main_reqs,
            "项目浏览器 zcode 今日请求数（{zcode_requests}）必须与主库直查（{main_reqs}）一致"
        );
        eprintln!(
            "诊断: 今日 zcode tokens={zcode_tokens} requests={zcode_requests}（与主库一致，差异 0%），\
             已归属真实项目的会话数={mapped_sessions}"
        );
    }
}

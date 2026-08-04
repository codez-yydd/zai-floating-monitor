use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// 单个模型在指定时间范围内的聚合统计
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelStat {
    pub model_id: String,
    pub provider_id: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
}

/// 整体统计（时间范围内汇总）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OverallStat {
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub cache_write_tokens: i64,
    pub reasoning_tokens: i64,
    pub total_tokens: i64,
}

/// get_stats 命令返回的完整结构
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    pub from_ms: i64,
    pub to_ms: i64,
    pub overall: OverallStat,
    pub by_model: Vec<ModelStat>,
    /// 数据库中实际有数据的最早/最晚时间（用于判断是否有数据）
    pub earliest_ms: Option<i64>,
    pub latest_ms: Option<i64>,
}

/// 价格表中的一个模型的单价（每百万 token）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub provider_id: String,
}

/// 定位 ZCode 的 SQLite 数据库路径。
/// 优先级：环境变量 ZBAR_DB > ~/.zcode/cli/db/db.sqlite
pub fn db_path() -> Result<PathBuf, String> {
    if let Ok(p) = std::env::var("ZBAR_DB") {
        let pb = PathBuf::from(p);
        if pb.exists() {
            return Ok(pb);
        }
    }
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    let p = home.join(".zcode/cli/db/db.sqlite");
    if p.exists() {
        Ok(p)
    } else {
        Err(format!(
            "未找到 ZCode 数据库: {}。请确认 ZCode 已安装，或设置 ZBAR_DB 环境变量。",
            p.display()
        ))
    }
}

/// 以只读方式打开数据库，避免干扰 ZCode 的写入。
fn open_db() -> Result<Connection, String> {
    let path = db_path()?;
    let conn = Connection::open_with_flags(
        &path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|e| format!("打开数据库失败: {e}"))?;
    // 即使只读，WAL 模式下读取也需要等待写锁释放
    conn.busy_timeout(std::time::Duration::from_secs(3))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// 查询指定时间范围 [from_ms, to_ms] 内的统计（时间均为毫秒时间戳）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<Stats, String> {
    let conn = open_db()?;

    // 整体汇总
    let overall: OverallStat = conn
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
                Ok(OverallStat {
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
        .map_err(|e| format!("查询整体统计失败: {e}"))?;

    // 按模型分组
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
        .map_err(|e| format!("准备模型分组查询失败: {e}"))?;

    let by_model = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok(ModelStat {
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
        .map_err(|e| format!("读取模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取模型分组失败: {e}"))?;

    // 数据时间范围
    let (earliest_ms, latest_ms): (Option<i64>, Option<i64>) = conn
        .query_row(
            "SELECT MIN(started_at), MAX(started_at) FROM model_usage",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .map_err(|e| format!("查询时间范围失败: {e}"))?;

    Ok(Stats {
        from_ms,
        to_ms,
        overall,
        by_model,
        earliest_ms,
        latest_ms,
    })
}

/// 列出数据库中出现过的所有 (provider_id, model_id) 组合，供价格配置用。
pub fn list_models() -> Result<Vec<ModelInfo>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT DISTINCT provider_id, model_id
             FROM model_usage
             ORDER BY model_id",
        )
        .map_err(|e| format!("准备模型列表查询失败: {e}"))?;

    let models = stmt
        .query_map([], |row| {
            Ok(ModelInfo {
                provider_id: row.get(0)?,
                model_id: row.get(1)?,
            })
        })
        .map_err(|e| format!("读取模型列表失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取模型列表失败: {e}"))?;

    Ok(models)
}

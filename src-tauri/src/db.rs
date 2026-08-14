use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
// TimeZone trait 提供 timestamp_millis_opt 等方法，用于把毫秒转回本地时间
use chrono::TimeZone;

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
pub(crate) fn open_db() -> Result<Connection, String> {
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

// ===== 增量查询（多设备同步用） =====

/// 单条明细记录（供同步上传用）。
/// 字段与 zcode 的 model_usage 表对齐，多带一个 local_rowid 作为去重键。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UsageRow {
    pub local_rowid: i64,
    pub started_at: i64,
    #[serde(default)]
    pub model_id: String,
    #[serde(default)]
    pub provider_id: String,
    #[serde(default)]
    pub input_tokens: i64,
    #[serde(default)]
    pub output_tokens: i64,
    #[serde(default)]
    pub cache_read_input_tokens: i64,
    #[serde(default)]
    pub cache_creation_input_tokens: i64,
    #[serde(default)]
    pub reasoning_tokens: i64,
    #[serde(default)]
    pub computed_total_tokens: i64,
}

/// 查询 rowid > since 的明细记录（增量上传用）。
/// 只读连接也能 SELECT rowid。按 rowid 升序，便于游标推进。
pub fn query_since(since: i64, limit: usize) -> Result<Vec<UsageRow>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT rowid, started_at, model_id, provider_id,
                    COALESCE(input_tokens,0), COALESCE(output_tokens,0),
                    COALESCE(cache_read_input_tokens,0), COALESCE(cache_creation_input_tokens,0),
                    COALESCE(reasoning_tokens,0), COALESCE(computed_total_tokens,0)
             FROM model_usage
             WHERE rowid > ?1
             ORDER BY rowid ASC
             LIMIT ?2",
        )
        .map_err(|e| format!("准备增量查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![since, limit as i64], |row| {
            Ok(UsageRow {
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
            })
        })
        .map_err(|e| format!("读取增量记录失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取增量记录失败: {e}"))?;
    Ok(rows)
}

/// 当前本地库的最大 rowid（供「待上传条数」显示用）。
pub fn max_rowid() -> Result<i64, String> {
    let conn = open_db()?;
    let max: i64 = conn
        .query_row("SELECT COALESCE(MAX(rowid), 0) FROM model_usage", [], |row| {
            row.get(0)
        })
        .map_err(|e| format!("查询最大 rowid 失败: {e}"))?;
    Ok(max)
}

// ===== 时间序列分桶聚合（趋势图用） =====

/// 某个桶内某模型的聚合。计费所需字段齐全，供 lib.rs 计算 cost。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BucketModelStat {
    pub model_id: String,
    pub provider_id: String,
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_tokens: i64,
}

/// 单个桶的原始聚合结果（db 层不含花费，cost 在 lib.rs 结合 pricing 计算）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendBucketRaw {
    /// 桶标签："14:00"（小时桶）或 "08-04"（日桶）
    pub label: String,
    /// 桶内按模型聚合（用于算 cost）
    pub by_model: Vec<BucketModelStat>,
    /// 桶内总 token
    pub total_tokens: i64,
    /// 桶内总请求数
    pub requests: i64,
}

/// 把毫秒时间戳对齐到桶起点。
/// - hour：对齐到所在小时的整点（按 UTC 毫秒取整，配合本地时区偏移）
/// - day ：对齐到本地 0 点
fn align_bucket_start(ms: i64, bucket: &str) -> i64 {
    if bucket == "hour" {
        // 1 小时 = 3600000ms，直接按整除对齐到整点。
        // started_at 是 UTC 毫秒，整点对齐后用本地时区格式化标签，
        // 因此桶边界与本地时钟的整点是一致的。
        (ms / 3_600_000) * 3_600_000
    } else {
        // 本地 0 点对齐：取本地日期，重设为 0 点。
        chrono::Local
            .timestamp_millis_opt(ms)
            .single()
            .map(|d| {
                d.date_naive()
                    .and_hms_opt(0, 0, 0)
                    .unwrap()
                    .and_local_timezone(chrono::Local)
                    .single()
                    .map(|dt| dt.timestamp_millis())
                    .unwrap_or(ms)
            })
            .unwrap_or(ms)
    }
}

/// 桶起始毫秒 → 标签字符串。
fn bucket_label(start_ms: i64, bucket: &str) -> String {
    chrono::Local
        .timestamp_millis_opt(start_ms)
        .single()
        .map(|d| {
            if bucket == "hour" {
                d.format("%H:00").to_string()
            } else {
                d.format("%m-%d").to_string()
            }
        })
        .unwrap_or_default()
}

/// 查询 [from_ms, to_ms) 内的分桶统计。
///
/// `bucket` 为 "hour" 或 "day"。采用逐桶循环查询：
/// 把 from 对齐到桶起点，按桶宽逐步推进直到覆盖 to。
/// 桶数 = (to - aligned_from) / 桶宽，通常 ≤31（日）或 ≤24（小时），开销可接受。
pub fn query_trend(
    from_ms: i64,
    to_ms: i64,
    bucket: &str,
) -> Result<Vec<TrendBucketRaw>, String> {
    let conn = open_db()?;
    let width = if bucket == "hour" { 3_600_000 } else { 86_400_000 };

    let mut start = align_bucket_start(from_ms, bucket);
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
        .map_err(|e| format!("准备趋势查询失败: {e}"))?;

    let mut out: Vec<TrendBucketRaw> = Vec::new();
    while start < to_ms {
        let end = start + width;
        // 查询区间与桶对齐；最后一桶的 end 可能超过 to_ms，但 SQL 用 < end，
        // 而 to_ms 之后的本就没有数据，不影响结果。
        let by_model: Vec<BucketModelStat> = stmt
            .query_map(rusqlite::params![start, end], |row| {
                Ok(BucketModelStat {
                    model_id: row.get(0)?,
                    provider_id: row.get(1)?,
                    requests: row.get(2)?,
                    input_tokens: row.get(3)?,
                    output_tokens: row.get(4)?,
                    cache_read_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                })
            })
            .map_err(|e| format!("读取趋势统计失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取趋势统计失败: {e}"))?;

        let total_tokens = by_model.iter().map(|m| m.total_tokens).sum();
        let requests = by_model.iter().map(|m| m.requests).sum();

        out.push(TrendBucketRaw {
            label: bucket_label(start, bucket),
            by_model,
            total_tokens,
            requests,
        });

        start = end;
    }

    Ok(out)
}

// ===== 按周期分桶聚合（对比页用）=====

/// 单个周期的 token 聚合结果。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeriodBucket {
    /// 周期开始（重置时间）
    pub reset_at: i64,
    /// 周期结束时间
    pub end_at: i64,
    /// 桶内总 token
    pub total_tokens: i64,
    /// 桶内总请求数
    pub requests: i64,
}

/// 对一组 [reset_at, end_at) 周期，逐周期聚合本地 model_usage 的 token。
/// 用于对比页"实际 token"列（本地部分，远端部分由前端调用 sync 合并）。
pub fn query_period_buckets(periods: &[(i64, i64)]) -> Result<Vec<PeriodBucket>, String> {
    let conn = open_db()?;
    let mut stmt = conn
        .prepare(
            "SELECT
                COALESCE(SUM(computed_total_tokens),0),
                COUNT(*)
             FROM model_usage
             WHERE started_at >= ?1 AND started_at < ?2",
        )
        .map_err(|e| format!("准备周期聚合查询失败: {e}"))?;

    let mut out = Vec::with_capacity(periods.len());
    for &(reset_at, end_at) in periods {
        let (total_tokens, requests): (i64, i64) = stmt
            .query_row(rusqlite::params![reset_at, end_at], |row| {
                Ok((row.get(0)?, row.get(1)?))
            })
            .map_err(|e| format!("查询周期聚合失败: {e}"))?;
        out.push(PeriodBucket {
            reset_at,
            end_at,
            total_tokens,
            requests,
        });
    }
    Ok(out)
}

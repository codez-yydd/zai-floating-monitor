use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
// TimeZone trait 提供 timestamp_millis_opt 等方法，用于把毫秒转回本地时间
use chrono::TimeZone;

/// 速度/延迟指标（serde flatten 平铺进 ModelStat/OverallStat，JSON 形状与
/// 直接加字段一致）。仅数据源带耗时的 Agent 有值（zcode 库有 duration+TTFT、
/// Claude 导入库有 duration；Codex/Cursor 无耗时数据恒为 None）。
/// 同步链路里旧版本数据无这些字段，反序列化按 None 兜底。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SpeedMetrics {
    /// 平均输出速度（tok/s，仅统计可信样本）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_tps: Option<f64>,
    /// 最快一次输出速度（tok/s）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_tps: Option<f64>,
    /// 平均首字延迟（毫秒，仅 zcode 库有 TTFT 数据）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub avg_ttft_ms: Option<f64>,
}

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
    #[serde(flatten)]
    pub speed: SpeedMetrics,
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
    #[serde(flatten)]
    pub speed: SpeedMetrics,
}

/// 最近使用的模型（口径：全库最新一条用量记录，非配置态的"当前选中"）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentModelStat {
    pub model_id: String,
    pub provider_id: String,
    /// 最近一次使用时间（毫秒时间戳）
    pub last_used_ms: i64,
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
    /// 最近使用的模型（与查询时间范围无关，取全库最新）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_model: Option<CurrentModelStat>,
}

/// 价格表中的一个模型的单价（每百万 token）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub model_id: String,
    pub provider_id: String,
}

/// 定位 ZCode 的 SQLite 数据库路径。
/// 优先级：环境变量 ZBAR_DB > ~/.zcode/cli/db/db.sqlite
/// 注意：CLI 数据库不跟随 ZCode 桌面端的「更改数据目录」（setting.json 的
/// dataBaseDir）迁移，始终在用户主目录下；若未来 ZCode 版本改变该行为，
/// 需要与 quota::zcode_v2_dir 的迁移解析对齐。
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

/// 探测表列是否存在（速度聚合按列有无动态降级；table 为代码内常量，无注入风险）。
pub(crate) fn has_column(conn: &Connection, table: &str, column: &str) -> bool {
    let sql = format!("SELECT COUNT(*) FROM pragma_table_info('{table}') WHERE name = ?1");
    conn.query_row(&sql, [column], |row| row.get::<_, i64>(0))
        .map(|c| c > 0)
        .unwrap_or(false)
}

// ===== 输出速度（tok/s）与首字延迟（TTFT）聚合 =====
//
// 口径参考 zcode-assistant 的噪声过滤：
// - 生成窗口 = 总耗时 − 首 token 等待（TTFT），即真实吐字窗口；
// - 「整块下发」（TTFT ≥ 90% 总耗时，非流式接口/中转缓冲把响应攒在服务端）：
//   总耗时 − TTFT 只是传输耗时，会把速度放大到数千 tok/s，改用 TTFT 本身
//   作为生成窗口（含排队与预填充，是速度的保守下界）；
// - 可信样本条件：输出 ≥10 tokens、生成窗口 ≥100ms、速度 ≤500 tok/s
//   （现有模型物理上达不到 500，超出视为计时异常丢弃，不参与均值）；
// - TTFT 均值取正值行（负值/缺失忽略）。

/// 行级生成窗口的 SQL CASE 表达式（引用列 duration_ms / time_to_first_token_ms）。
/// has_ttft = false（无 TTFT 列，如 Claude 导入库）：生成窗口退化为总耗时。
fn gen_window_expr(has_ttft: bool) -> String {
    if !has_ttft {
        return "CASE WHEN COALESCE(duration_ms,0) > 0 THEN duration_ms END".into();
    }
    "CASE
        WHEN COALESCE(duration_ms,0) <= 0 THEN NULL
        WHEN time_to_first_token_ms IS NOT NULL
             AND time_to_first_token_ms >= 0
             AND time_to_first_token_ms <= duration_ms
        THEN CASE
            WHEN time_to_first_token_ms * 10 >= duration_ms * 9
            THEN time_to_first_token_ms
            ELSE duration_ms - time_to_first_token_ms
        END
        ELSE duration_ms
    END"
    .into()
}

/// 速度/TTFT 聚合的 SELECT 尾部片段（3 列：avg_tps, max_tps, avg_ttft_ms）。
/// 列缺失时全部占位 NULL（如 Codex 导入库无耗时数据）。
/// 追加在既有聚合 SQL 的最后一个聚合列之后，读取方按 Option<f64> 取列。
pub(crate) fn speed_agg_columns(has_duration: bool, has_ttft: bool) -> String {
    if !has_duration {
        return ", NULL, NULL, NULL".into();
    }
    let gen = gen_window_expr(has_ttft);
    let tps = format!(
        "CASE
            WHEN COALESCE(output_tokens,0) >= 10 AND ({gen}) >= 100
                 AND COALESCE(output_tokens,0) * 1000.0 / ({gen}) <= 500.0
            THEN COALESCE(output_tokens,0) * 1000.0 / ({gen})
        END"
    );
    let ttft = if has_ttft {
        // TTFT 物理上不超过总耗时，超出视为异常计时一并忽略
        "CASE WHEN time_to_first_token_ms >= 0
               AND time_to_first_token_ms <= duration_ms
          THEN time_to_first_token_ms END"
    } else {
        "NULL"
    };
    format!(", AVG({tps}), MAX({tps}), AVG({ttft})")
}

/// 最近使用模型查询（全库最新一条；空模型名的行跳过，表空返回 None）。
pub(crate) fn query_current_model(conn: &Connection) -> Option<CurrentModelStat> {
    conn.query_row(
        "SELECT model_id, provider_id, started_at
         FROM model_usage
         WHERE model_id IS NOT NULL AND model_id != ''
         ORDER BY started_at DESC LIMIT 1",
        [],
        |row| {
            Ok(CurrentModelStat {
                model_id: row.get::<_, Option<String>>(0)?.unwrap_or_default(),
                provider_id: row.get::<_, Option<String>>(1)?.unwrap_or_default(),
                last_used_ms: row.get(2)?,
            })
        },
    )
    .ok()
}

/// 查询指定时间范围 [from_ms, to_ms] 内的统计（时间均为毫秒时间戳）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<Stats, String> {
    let conn = open_db()?;
    // zcode 库带 duration/TTFT 列；其它模块的导入库按列有无自动降级
    let speed = speed_agg_columns(
        has_column(&conn, "model_usage", "duration_ms"),
        has_column(&conn, "model_usage", "time_to_first_token_ms"),
    );

    // 整体汇总
    let overall: OverallStat = conn
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
                Ok(OverallStat {
                    requests: row.get(0)?,
                    input_tokens: row.get(1)?,
                    output_tokens: row.get(2)?,
                    cache_read_tokens: row.get(3)?,
                    cache_write_tokens: row.get(4)?,
                    reasoning_tokens: row.get(5)?,
                    total_tokens: row.get(6)?,
                    speed: SpeedMetrics {
                        avg_tps: row.get(7)?,
                        max_tps: row.get(8)?,
                        avg_ttft_ms: row.get(9)?,
                    },
                })
            },
        )
        .map_err(|e| format!("查询整体统计失败: {e}"))?;

    // 按模型分组
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
                speed: SpeedMetrics {
                    avg_tps: row.get(9)?,
                    max_tps: row.get(10)?,
                    avg_ttft_ms: row.get(11)?,
                },
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
        current_model: query_current_model(&conn),
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

/// source 字段缺省值：旧服务端/旧数据不区分来源，反序列化缺省按 zcode 处理。
pub(crate) fn default_source() -> String {
    "zcode".into()
}

/// 单条明细记录（供同步上传用）。
/// 字段与 zcode 的 model_usage 表对齐，多带一个 local_rowid 作为去重键。
/// source 标记数据来源："zcode"（本地 ZCode 库）| "codex"（Codex 导入库）。
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
    #[serde(default = "default_source")]
    pub source: String,
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
                // 本库（zcode）的行固定标记为 zcode 来源
                source: "zcode".into(),
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
/// codex 模块的趋势查询复用这两个函数，保持与 zcode 一致的桶边界。
pub(crate) fn align_bucket_start(ms: i64, bucket: &str) -> i64 {
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
pub(crate) fn bucket_label(start_ms: i64, bucket: &str) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 建一个模拟 zcode model_usage 的内存表（带 duration/TTFT 列）并返回连接
    fn zcode_like_db() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                input_tokens INTEGER DEFAULT 0, output_tokens INTEGER DEFAULT 0,
                cache_read_input_tokens INTEGER DEFAULT 0,
                cache_creation_input_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                computed_total_tokens INTEGER DEFAULT 0,
                duration_ms INTEGER, time_to_first_token_ms INTEGER
            );",
        )
        .unwrap();
        conn
    }

    fn insert(conn: &Connection, ms: i64, output: i64, dur: Option<i64>, ttft: Option<i64>) {
        conn.execute(
            "INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens,
                input_tokens, computed_total_tokens, duration_ms, time_to_first_token_ms)
             VALUES (?1, 'm', 'p', ?2, 0, ?2, ?3, ?4)",
            rusqlite::params![ms, output, dur, ttft],
        )
        .unwrap();
    }

    fn approx(a: f64, b: f64) -> bool {
        (a - b).abs() < 1e-6
    }

    /// 速度/TTFT 聚合口径：正常流式、整块下发、噪声过滤、TTFT 异常值忽略。
    #[test]
    fn speed_aggregation_semantics() {
        let conn = zcode_like_db();
        // 正常流式：生成窗口 = 2000-500 = 1500ms，300 tok → 200 tok/s
        insert(&conn, 1_000, 300, Some(2000), Some(500));
        // 整块下发：TTFT(1900) ≥ 90%×2000 → 窗口=1900，950 tok → 500 tok/s
        insert(&conn, 2_000, 950, Some(2000), Some(1900));
        // 输出 <10 tok：不可信，不计入
        insert(&conn, 3_000, 5, Some(2000), Some(500));
        // 生成窗口 50-10=40ms <100ms：计时噪声，不计入
        insert(&conn, 4_000, 100, Some(50), Some(10));
        // 10000 tok / 1s = 10000 tok/s >500：计时异常，不计入
        insert(&conn, 5_000, 10_000, Some(1000), Some(0));
        // TTFT 缺失：窗口退化为总耗时，300/3s = 100 tok/s
        insert(&conn, 6_000, 300, Some(3000), None);
        // TTFT > 总耗时（异常）：窗口退化总耗时 100 tok/s，TTFT 本身也不计均值
        insert(&conn, 7_000, 300, Some(3000), Some(4000));
        // 总耗时缺失：完全无速度信息
        insert(&conn, 8_000, 300, None, Some(100));

        let speed = speed_agg_columns(true, true);
        let (avg, max, ttft): (Option<f64>, Option<f64>, Option<f64>) = conn
            .query_row(
                &format!("SELECT COUNT(*) {speed} FROM model_usage"),
                [],
                |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        // 可信样本 = {200, 500, 100, 100}；TTFT 样本 = {500,1900,500,10,0}
        assert!(approx(avg.unwrap(), 225.0), "avg={avg:?}");
        assert!(approx(max.unwrap(), 500.0), "max={max:?}");
        assert!(approx(ttft.unwrap(), 582.0), "ttft={ttft:?}");
    }

    /// 无耗时列的库（Codex 导入库同构）：占位 NULL，查询不报错。
    #[test]
    fn speed_columns_missing_degrade_to_null() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                output_tokens INTEGER DEFAULT 0,
                input_tokens INTEGER DEFAULT 0,
                cache_read_input_tokens INTEGER DEFAULT 0,
                cache_creation_input_tokens INTEGER DEFAULT 0,
                reasoning_tokens INTEGER DEFAULT 0,
                computed_total_tokens INTEGER DEFAULT 0
            );
            INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens)
             VALUES (1000, 'm', 'p', 300);",
        )
        .unwrap();
        assert!(!has_column(&conn, "model_usage", "duration_ms"));
        let speed = speed_agg_columns(false, false);
        let (avg, max, ttft): (Option<f64>, Option<f64>, Option<f64>) = conn
            .query_row(
                &format!("SELECT COUNT(*) {speed} FROM model_usage"),
                [],
                |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?)),
            )
            .unwrap();
        assert_eq!((avg, max, ttft), (None, None, None));
    }

    /// 最近使用模型：取 started_at 最新一条（跳过空模型名行）；空表为 None。
    #[test]
    fn current_model_takes_latest_row() {
        let conn = zcode_like_db();
        assert!(query_current_model(&conn).is_none());
        insert(&conn, 1_000, 10, None, None);
        insert(&conn, 9_000, 10, None, None);
        insert(&conn, 5_000, 10, None, None);
        let cur = query_current_model(&conn).unwrap();
        assert_eq!(cur.last_used_ms, 9_000);
        assert_eq!(cur.model_id, "m");
        assert_eq!(cur.provider_id, "p");
    }

    /// 最新一条恰好是空模型名（codex 回填失败场景）：跳过它取次新的有效模型。
    #[test]
    fn current_model_skips_empty_model_rows() {
        let conn = zcode_like_db();
        conn.execute(
            "INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens)
             VALUES (9000, '', 'p', 10)",
            [],
        )
        .unwrap();
        insert(&conn, 5_000, 10, None, None);
        let cur = query_current_model(&conn).unwrap();
        assert_eq!(cur.last_used_ms, 5_000);
        // 全库只有空模型行 → None
        let conn2 = zcode_like_db();
        conn2
            .execute(
                "INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens)
                 VALUES (9000, '', 'p', 10)",
                [],
            )
            .unwrap();
        assert!(query_current_model(&conn2).is_none());
    }
}

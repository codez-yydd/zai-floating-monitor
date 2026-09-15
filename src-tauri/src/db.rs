use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::PathBuf;
// TimeZone trait 提供 timestamp_millis_opt 等方法，用于把毫秒转回本地时间
use chrono::TimeZone;

/// 速度/延迟指标（serde flatten 平铺进 ModelStat/OverallStat，JSON 形状与
/// 直接加字段一致）。仅数据源带耗时的 Agent 有值（zcode 库有 duration+TTFT、
/// Claude 导入库有 duration；Codex/Cursor 无耗时数据恒为 None）。
/// 同步链路里旧版本数据无这些字段，反序列化按 None 兜底。
///
/// ZCode 主库（db::query_stats）自 V24.1 起改用**真实事件时间**的生成口径
/// （first_token_at → completed_at，共享 token_speed::generation_sample 行级
/// 判定）：`avg_tps = speedOutputTokens × 1000 / speedGenerationMs`（分母为
/// 零 → null，绝不显示 0 t/s 冒充缺失），`max_tps` 为合格样本中单请求最快
/// 值（无 500 t/s 封顶）。`speedQuality` 区分口径：zcode 恒为 `generation`
/// （有合格样本时；历史页面方案只展示 generation 样本，首 Token 缺失的
/// request_average 参考值不混入）；claude/kimi 只有请求总耗时，标
/// `request_average`（TS 侧用 ≈ 前缀区分）。`avg_ttft_ms` 是独立指标，
/// 有自己的样本集合（`ttftSampleCount`），不与速度样本同集。
/// 旧字段（avg_tps/max_tps/avg_ttft_ms）保留原名（snake 形态，前端既有
/// 消费端不动），新字段为 camelCase 契约：speedOutputTokens、
/// speedGenerationMs、speedSampleCount、ttftSampleCount、speedQuality。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
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
    /// 速度合格样本的输出 token 合计（可加总分子；无合格样本为 None）
    #[serde(
        rename = "speedOutputTokens",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub speed_output_tokens: Option<i64>,
    /// 速度合格样本的生成毫秒合计（可加总分母；无合格样本为 None）
    #[serde(
        rename = "speedGenerationMs",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub speed_generation_ms: Option<i64>,
    /// 速度合格样本数（请求笔数口径之外的独立计数）
    #[serde(
        rename = "speedSampleCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub speed_sample_count: Option<i64>,
    /// TTFT 有效样本数（avg_ttft_ms 自己的样本集合，与速度样本独立）
    #[serde(
        rename = "ttftSampleCount",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub ttft_sample_count: Option<i64>,
    /// 速度口径质量标记：generation（可信首字到完成）/ request_average
    /// （仅请求总耗时的近似，界面带 ≈）。zcode 有合格样本时为 generation，
    /// 否则 None；claude/kimi 有速度时恒为 request_average。
    #[serde(
        rename = "speedQuality",
        default,
        skip_serializing_if = "Option::is_none"
    )]
    pub(crate) speed_quality: Option<crate::token_speed::SpeedQuality>,
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

/// 会话级速度/TTFT 聚合的 SELECT 尾部片段（4 列：可信行 tps 总和、可信行数、
/// 有效 TTFT 行总和、有效 TTFT 行数）。供项目浏览器会话查询用：行查询按
/// 「会话 × 模型」分组，跨分组保持与 speed_agg_columns 相同的「整体平均」
/// 口径必须传 SUM/COUNT（各分组 SUM/COUNT 分别累加后再相除 = 全部可信行的
/// AVG，直接对分组 AVG 再平均会产生二次平均偏差）。
/// 可信条件与 speed_agg_columns 完全一致（同一 gen_window_expr 与噪声过滤：
/// 输出 ≥10 tokens、生成窗口 ≥100ms、速度 ≤500 tok/s；TTFT 取 0≤ttft≤duration），
/// 两处表达式需同步维护。列缺失时占位 NULL/0（无耗时来源 → 调用方读得 None）。
pub(crate) fn session_speed_agg_columns(has_duration: bool, has_ttft: bool) -> String {
    if !has_duration {
        return ", NULL, 0, NULL, 0".into();
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
        // TTFT 物理上不超过总耗时，超出视为异常计时一并忽略（同 speed_agg_columns）
        "CASE WHEN time_to_first_token_ms >= 0
               AND time_to_first_token_ms <= duration_ms
          THEN time_to_first_token_ms END"
    } else {
        "NULL"
    };
    format!(", SUM({tps}), COUNT({tps}), SUM({ttft}), COUNT({ttft})")
}

/// 会话级 TTFT-only 聚合片段（2 列：有效 TTFT 行总和、行数）。供 zcode
/// 项目会话查询用——速度部分已切换为行级生成口径（B4，Rust 侧
/// token_speed::generation_sample 聚合），TTFT 是独立指标沿用列表达式；
/// Claude/Kimi/Codex 继续使用 session_speed_agg_columns 的完整片段。
pub(crate) fn session_ttft_agg_columns(has_duration: bool, has_ttft: bool) -> String {
    if !has_duration || !has_ttft {
        return ", NULL, 0".into();
    }
    let ttft = "CASE WHEN time_to_first_token_ms >= 0 \
               AND time_to_first_token_ms <= duration_ms \
          THEN time_to_first_token_ms END";
    format!(", SUM({ttft}), COUNT({ttft})")
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

/// ZCode 主库行级生成速度聚合的可加总中间值（V24.1 起替代
/// query_stats 路径的旧 speed_agg_columns 算法；Claude/Kimi 导入库仍走
/// 旧口径并标 request_average）。`[from,to)` 过滤按 started_at，与既有
/// 语义一致。
#[derive(Default, Debug, Clone)]
pub(crate) struct GenerationSpeedAgg {
    /// 合格样本输出 token 合计（分子）
    speed_output_tokens: i64,
    /// 合格样本生成毫秒合计（分母）
    speed_generation_ms: i64,
    /// 合格样本数
    speed_sample_count: i64,
    /// 单请求最快生成速度
    max_tps: f64,
    /// TTFT 有效样本合计（独立口径，复用旧规则的 0 ≤ ttft ≤ duration）
    ttft_sum_ms: i64,
    /// TTFT 有效样本数
    ttft_sample_count: i64,
}

impl GenerationSpeedAgg {
    fn absorb(&mut self, sample: &crate::token_speed::GenerationSample) {
        self.speed_output_tokens += sample.output_tokens;
        self.speed_generation_ms += sample.generation_ms;
        self.speed_sample_count += 1;
        if sample.tps > self.max_tps {
            self.max_tps = sample.tps;
        }
    }

    fn absorb_ttft(&mut self, ttft_ms: i64) {
        self.ttft_sum_ms += ttft_ms;
        self.ttft_sample_count += 1;
    }

    /// 组装为对外契约。分母为零（无合格样本）时全部速度字段为 None——
    /// 绝不以 0 t/s 冒充缺失。
    fn into_metrics(self) -> SpeedMetrics {
        let has_generation = self.speed_sample_count > 0 && self.speed_generation_ms > 0;
        SpeedMetrics {
            avg_tps: has_generation.then(|| {
                self.speed_output_tokens as f64 * 1000.0 / self.speed_generation_ms as f64
            }),
            max_tps: (self.speed_sample_count > 0).then_some(self.max_tps),
            avg_ttft_ms: (self.ttft_sample_count > 0)
                .then(|| self.ttft_sum_ms as f64 / self.ttft_sample_count as f64),
            speed_output_tokens: (self.speed_sample_count > 0).then_some(self.speed_output_tokens),
            speed_generation_ms: (self.speed_sample_count > 0).then_some(self.speed_generation_ms),
            speed_sample_count: (self.speed_sample_count > 0).then_some(self.speed_sample_count),
            ttft_sample_count: (self.ttft_sample_count > 0).then_some(self.ttft_sample_count),
            // ZCode 历史页面方案：仅展示 generation 合格样本；首 Token 缺失
            // 的 request_average 参考值不混入聚合（缺失即 null，显示 —）
            speed_quality: (self.speed_sample_count > 0)
                .then_some(crate::token_speed::SpeedQuality::Generation),
        }
    }
}

/// ZCode 主库专属的行级生成速度聚合（B2）：读 `model_usage` 的
/// status/started_at/first_token_at/completed_at/output_tokens，用共享的
/// token_speed::generation_sample 行级判定验证（真实生成样本要求
/// output>0 且 completed>first>0，外加 started 校验与未来时间拒绝；
/// 合法 <100ms、>500 t/s 的行不因阈值丢弃），在 Rust 侧累加 overall 与
/// 各 (provider_id, model_id) 的可加总分子/分母/样本数/max。TTFT 沿用
/// time_to_first_token_ms 列的独立口径（0 ≤ ttft ≤ duration 的行）。
/// 老库缺列（无 status/first_token_at/completed_at）安全降级：按已完成
/// 表处理 / 无生成样本返回空聚合（速度字段 None）。
pub(crate) fn collect_generation_speed(
    conn: &Connection,
    from_ms: i64,
    to_ms: i64,
    observed_at_ms: i64,
) -> Result<(SpeedMetrics, BTreeMap<(String, String), SpeedMetrics>), String> {
    let mut overall = GenerationSpeedAgg::default();
    let mut by_model: BTreeMap<(String, String), GenerationSpeedAgg> = BTreeMap::new();
    let opt = |col: &str| {
        if has_column(conn, "model_usage", col) {
            col.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let num = |col: &str| {
        if has_column(conn, "model_usage", col) {
            format!("COALESCE({col}, 0)")
        } else {
            "0".to_string()
        }
    };
    let (status, first, completed, ttft, duration, output) = (
        opt("status"),
        opt("first_token_at"),
        opt("completed_at"),
        opt("time_to_first_token_ms"),
        opt("duration_ms"),
        num("output_tokens"),
    );
    let sql = format!(
        "SELECT COALESCE(provider_id, ''), COALESCE(model_id, ''), {status}, \
                started_at, {first}, {completed}, {output}, {ttft}, {duration} \
         FROM model_usage \
         WHERE started_at >= ?1 AND started_at < ?2"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备生成速度查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![from_ms, to_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, Option<i64>>(7)?,
                row.get::<_, Option<i64>>(8)?,
            ))
        })
        .map_err(|e| format!("读取生成速度行失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取生成速度行失败: {e}"))?;

    for (provider, model, status, started, first, completed, output, ttft, duration) in rows {
        // 完成判定：status 列缺失（老库完成即落行）或空值按完成降级
        if !crate::token_speed::is_completed_status(status.as_deref()) {
            continue;
        }
        let timing = crate::token_speed::RequestTiming {
            output_tokens: output.max(0),
            started_at: started.filter(|value| *value > 0),
            first_token_at: first,
            completed_at: completed,
            duration_ms: None,
            request_id: None,
        };
        if let Some(sample) = crate::token_speed::generation_sample(&timing, observed_at_ms) {
            overall.absorb(&sample);
            if !model.is_empty() {
                by_model
                    .entry((provider.clone(), model.clone()))
                    .or_default()
                    .absorb(&sample);
            }
        }
        // TTFT 独立口径：列值非负且不超过 duration（duration 缺失的行与旧
        // SQL 表达式一致不参与）
        if let Some(ttft) = ttft.filter(|value| *value >= 0) {
            if duration.is_some_and(|dur| ttft <= dur) {
                overall.absorb_ttft(ttft);
                if !model.is_empty() {
                    by_model
                        .entry((provider, model))
                        .or_default()
                        .absorb_ttft(ttft);
                }
            }
        }
    }
    Ok((
        overall.into_metrics(),
        by_model
            .into_iter()
            .map(|(key, agg)| (key, agg.into_metrics()))
            .collect(),
    ))
}

/// 查询指定时间范围 [from_ms, to_ms] 内的统计（时间均为毫秒时间戳）。
/// V24.1 起 ZCode 主库的速度/TTFT 由 collect_generation_speed 行级聚合
/// （真实事件时间 + 可加总分子分母），不再使用 speed_agg_columns 的
/// duration−TTFT 估算法（该函数保留给 Claude/Kimi 导入库）。
pub fn query_stats(from_ms: i64, to_ms: i64) -> Result<Stats, String> {
    let conn = open_db()?;
    let observed_at_ms = chrono::Utc::now().timestamp_millis();
    let (overall_speed, by_model_speed) =
        collect_generation_speed(&conn, from_ms, to_ms, observed_at_ms)?;

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
                    speed: overall_speed,
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

    let mut by_model = stmt
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
                speed: SpeedMetrics::default(),
            })
        })
        .map_err(|e| format!("读取模型分组失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取模型分组失败: {e}"))?;
    for model in &mut by_model {
        let key = (model.provider_id.clone(), model.model_id.clone());
        model.speed = by_model_speed
            .get(&key)
            .cloned()
            .unwrap_or_default();
    }

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
/// proto 5 起每条记录额外携带会话/项目维度。三字段由 sync 模块按来源
/// 填充：zcode 源按 zcode_sessions 派生库的 session_id → 项目映射回填，
/// codex/claude 源由各自导入库填充；查不到映射的行保持 None。旧服务端
/// 反序列化时按 serde 默认忽略未知字段，序列化端 None 时跳过不发，
/// 双向兼容 proto 2/3/4。
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
    /// 该行的会话 id（无会话维度的来源为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// 归一化项目键（无项目维度为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_key: Option<String>,
    /// 原始形态 cwd（保留大小写，供前端展示；无则为 None）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub project_display: Option<String>,
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
                // 本查询不回填维度；zcode 行的三字段由
                // sync::query_zcode_rows_with_sessions 按派生库映射填充后上传
                session_id: None,
                project_key: None,
                project_display: None,
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

    /// 会话级速度聚合片段（session_speed_agg_columns）：SUM/COUNT 口径与
    /// 面板级 speed_agg_columns 的 AVG 完全等价（同样本同结果），跨分组
    /// （会话 × 模型）分别累加 SUM/COUNT 后相除仍等于整体 AVG。
    #[test]
    fn session_speed_agg_matches_panel_avg() {
        let conn = zcode_like_db();
        // 与 speed_aggregation_semantics 相同的样本：可信 avg = 225、ttft = 582
        insert(&conn, 1_000, 300, Some(2000), Some(500));
        insert(&conn, 2_000, 950, Some(2000), Some(1900));
        insert(&conn, 3_000, 5, Some(2000), Some(500));
        insert(&conn, 4_000, 100, Some(50), Some(10));
        insert(&conn, 5_000, 10_000, Some(1000), Some(0));
        insert(&conn, 6_000, 300, Some(3000), None);
        insert(&conn, 7_000, 300, Some(3000), Some(4000));
        insert(&conn, 8_000, 300, None, Some(100));

        let speed = session_speed_agg_columns(true, true);
        // 模拟项目浏览器真实路径：SQL 按「会话 × 模型」（此处按 model_id）
        // 分组产出各分组 SUM/COUNT，Rust 侧累加后相除得会话级平均
        let mut stmt = conn
            .prepare(&format!(
                "SELECT model_id{speed} FROM model_usage GROUP BY model_id"
            ))
            .unwrap();
        let mut rows = stmt.query([]).unwrap();
        let (mut sum, mut cnt, mut tsum, mut tcnt) = (0.0f64, 0i64, 0.0f64, 0i64);
        while let Some(r) = rows.next().unwrap() {
            if let Some(s) = r.get::<_, Option<f64>>(1).unwrap() {
                sum += s;
            }
            cnt += r.get::<_, i64>(2).unwrap();
            if let Some(s) = r.get::<_, Option<f64>>(3).unwrap() {
                tsum += s;
            }
            tcnt += r.get::<_, i64>(4).unwrap();
        }
        let panel_avg = 225.0f64;
        let panel_ttft = 582.0f64;
        assert!(cnt > 0 && (sum / cnt as f64 - panel_avg).abs() < 1e-6);
        assert!(tcnt > 0 && (tsum / tcnt as f64 - panel_ttft).abs() < 1e-6);
    }

    /// 无耗时列：会话级片段占位 NULL/0，调用方读得 None。
    #[test]
    fn session_speed_agg_missing_columns_degrade() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, model_id TEXT, provider_id TEXT,
                output_tokens INTEGER DEFAULT 0
            );
            INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens)
             VALUES (1000, 'm', 'p', 300);",
        )
        .unwrap();
        let speed = session_speed_agg_columns(false, false);
        let (sum, cnt, tsum, tcnt): (Option<f64>, i64, Option<f64>, i64) = conn
            .query_row(
                &format!("SELECT 1 AS one{speed} FROM model_usage"),
                [],
                |r| Ok((r.get(1)?, r.get(2)?, r.get(3)?, r.get(4)?)),
            )
            .unwrap();
        assert_eq!((sum, cnt, tsum, tcnt), (None, 0, None, 0));
    }

    /// V24.1 行级生成速度聚合测试库（带 status/first_token_at/completed_at）
    fn generation_db() -> Connection {
        let conn = zcode_like_db();
        conn.execute_batch(
            "ALTER TABLE model_usage ADD COLUMN status TEXT;
             ALTER TABLE model_usage ADD COLUMN first_token_at INTEGER;
             ALTER TABLE model_usage ADD COLUMN completed_at INTEGER;",
        )
        .unwrap();
        conn
    }

    /// 插入一行带事件时间的请求（first/completed 为 NULL 时用 None）
    fn insert_timed(
        conn: &Connection,
        ms: i64,
        model: &str,
        output: i64,
        first: Option<i64>,
        completed: Option<i64>,
        status: Option<&str>,
        ttft: Option<i64>,
        dur: Option<i64>,
    ) {
        conn.execute(
            "INSERT INTO model_usage (started_at, model_id, provider_id, output_tokens,
                input_tokens, computed_total_tokens, duration_ms, time_to_first_token_ms,
                status, first_token_at, completed_at)
             VALUES (?1, ?2, 'p', ?3, 0, ?3, ?4, ?5, ?6, ?7, ?8)",
            rusqlite::params![ms, model, output, dur, ttft, status, first, completed],
        )
        .unwrap();
    }

    /// 生成口径核心断言：1000 Token / 首 Token 到完成 8s = 125 t/s（TTFT
    /// 92s 不改用旧公式）；合法 <100ms、>500 t/s 不因阈值丢弃；逆序/未来/
    /// 未完成/零输出无速度；首 Token 缺失不算生成样本；TTFT 独立样本集。
    #[test]
    fn generation_speed_semantics() {
        let conn = generation_db();
        // 模型 A：1000 tok，first 92000 → completed 100000（生成 8s）→ 125
        insert_timed(
            &conn, 0, "A", 1000, Some(92_000), Some(100_000), Some("completed"), Some(92_000), Some(100_000),
        );
        // 模型 A：合法 10ms 短样本（>500 t/s，不封顶）：50 tok → 5000 t/s
        insert_timed(
            &conn, 1_000, "A", 50, Some(101_000), Some(101_010), Some("completed"), None, None,
        );
        // 模型 B：逆序（completed < first）→ 无样本
        insert_timed(
            &conn, 2_000, "B", 100, Some(103_000), Some(102_999), Some("completed"), None, None,
        );
        // 模型 B：未来时刻（completed 晚于观测时刻 110_000）→ 无样本
        insert_timed(
            &conn, 3_000, "B", 100, Some(119_000), Some(119_500), Some("completed"), None, None,
        );
        // 模型 B：未完成状态 → 无样本
        insert_timed(
            &conn, 4_000, "B", 100, Some(105_000), Some(105_500), Some("running"), None, None,
        );
        // 模型 B：零输出 → 无样本
        insert_timed(
            &conn, 5_000, "B", 0, Some(106_000), Some(106_500), Some("completed"), None, None,
        );
        // 模型 B：首 Token 缺失（duration 可用）→ 不混入生成均速
        insert_timed(&conn, 6_000, "B", 300, None, Some(107_000), Some("completed"), None, Some(3_000));
        // 模型 B：first 早于 started → 无样本（共享判定的补齐项）
        insert_timed(
            &conn, 7_000, "B", 100, Some(6_999), Some(108_000), Some("completed"), None, None,
        );
        // 模型 B：TTFT 异常（> duration）→ TTFT 不计入，但速度样本有效
        insert_timed(
            &conn, 8_000, "B", 200, Some(109_000), Some(109_400), Some("completed"), Some(5_000), Some(1_000),
        );

        let (overall, by_model) = collect_generation_speed(&conn, 0, 200_000, 110_000).unwrap();
        // overall：样本 = A 两笔（8s+10ms）+ B 的 109000→109400（400ms）→
        // 加权 (1000+50+200)×1000/(8000+10+400) ≈ 148.30
        assert_eq!(overall.speed_sample_count, Some(3));
        assert_eq!(overall.speed_output_tokens, Some(1_250));
        assert_eq!(overall.speed_generation_ms, Some(8_410));
        assert!(approx(overall.avg_tps.unwrap(), 1_250_000.0 / 8_410.0), "{overall:?}");
        assert!(approx(overall.max_tps.unwrap(), 5_000.0), "max 应取单请求最快且不封顶");
        assert_eq!(overall.speed_quality, Some(crate::token_speed::SpeedQuality::Generation));
        // TTFT 独立样本集：A 的 92000（≤ duration）与 B 的 5000（> duration
        // 剔除）、B 首字缺失行 TTFT 无值 → 仅 1 个样本
        assert_eq!(overall.ttft_sample_count, Some(1));
        assert!(approx(overall.avg_ttft_ms.unwrap(), 92_000.0));

        let a = by_model.get(&("p".to_string(), "A".to_string())).unwrap();
        assert!(approx(a.avg_tps.unwrap(), 1_050_000.0 / 8_010.0));
        let b = by_model.get(&("p".to_string(), "B".to_string())).unwrap();
        // B 仅 109000→109400 一笔有效：200 tok / 0.4s = 500 t/s
        assert_eq!(b.speed_sample_count, Some(1));
        assert!(approx(b.avg_tps.unwrap(), 500.0));
        assert_eq!(b.ttft_sample_count, None, "TTFT 异常行不计入（无样本为 None）");

        // 可加总性：总体分子/分母/样本数 = Σ模型
        let mut sum_out = 0;
        let mut sum_ms = 0;
        let mut sum_count = 0;
        for agg in by_model.values() {
            sum_out += agg.speed_output_tokens.unwrap_or(0);
            sum_ms += agg.speed_generation_ms.unwrap_or(0);
            sum_count += agg.speed_sample_count.unwrap_or(0);
        }
        assert_eq!(sum_out, overall.speed_output_tokens.unwrap());
        assert_eq!(sum_ms, overall.speed_generation_ms.unwrap());
        assert_eq!(sum_count, overall.speed_sample_count.unwrap());
    }

    /// 老库缺列（无 status/first_token_at/completed_at）安全降级：无生成
    /// 样本时全部速度字段 None（绝不是 0），TTFT 口径照旧可用。
    #[test]
    fn generation_speed_old_schema_degrades() {
        let conn = zcode_like_db();
        insert(&conn, 1_000, 300, Some(2000), Some(500));
        let (overall, by_model) = collect_generation_speed(&conn, 0, 100_000, 200_000).unwrap();
        assert_eq!(overall.avg_tps, None, "缺事件时间列不得伪造速度");
        assert_eq!(overall.max_tps, None);
        assert_eq!(overall.speed_sample_count, None);
        assert_eq!(overall.speed_output_tokens, None);
        assert_eq!(overall.speed_generation_ms, None);
        assert_eq!(overall.speed_quality, None);
        assert_eq!(overall.ttft_sample_count, Some(1), "TTFT 列仍在时独立口径可用");
        assert!(approx(overall.avg_ttft_ms.unwrap(), 500.0));
        let m = by_model.get(&("p".to_string(), "m".to_string())).unwrap();
        assert_eq!(m.avg_tps, None);
        assert_eq!(m.ttft_sample_count, Some(1));
    }

    /// 分母为零绝不产出 0 t/s；[from,to) 按 started_at 过滤保持既有语义。
    #[test]
    fn generation_speed_window_and_zero_denominator() {
        let conn = generation_db();
        insert_timed(
            &conn, 50_000, "A", 100, Some(50_100), Some(50_200), Some("completed"), None, None,
        );
        // 窗口外（started_at >= 100_000 才计入）
        insert_timed(
            &conn, 100_000, "A", 999, Some(100_100), Some(100_200), Some("completed"), None, None,
        );
        let (overall, _) = collect_generation_speed(&conn, 90_000, 200_000, 300_000).unwrap();
        assert_eq!(overall.speed_sample_count, Some(1));
        // 窗口内仅 started 100_000 的行：999 tok / 100ms → 9990 t/s（>500
        // 不封顶）
        assert!(approx(overall.avg_tps.unwrap(), 9_990.0));
        // 空窗口：全部 None（不是 0）
        let (empty, by_model) = collect_generation_speed(&conn, 500_000, 600_000, 700_000).unwrap();
        assert_eq!(empty.avg_tps, None);
        assert!(by_model.is_empty());
    }

    /// 新字段序列化契约：camelCase 键名 + None 时省略（flatten 进
    /// ModelStat/OverallStat 后的 JSON 形状与直接加字段一致）。
    #[test]
    fn speed_metrics_serialization_contract() {
        let metrics = SpeedMetrics {
            avg_tps: Some(63.1),
            max_tps: Some(1911.11),
            avg_ttft_ms: Some(812.5),
            speed_output_tokens: Some(1_234_567),
            speed_generation_ms: Some(19_568_000),
            speed_sample_count: Some(1_627),
            ttft_sample_count: Some(1_500),
            speed_quality: Some(crate::token_speed::SpeedQuality::Generation),
        };
        let json = serde_json::to_string(&metrics).unwrap();
        for key in [
            "\"avg_tps\":63.1",
            "\"max_tps\":1911.11",
            "\"avg_ttft_ms\":812.5",
            "\"speedOutputTokens\":1234567",
            "\"speedGenerationMs\":19568000",
            "\"speedSampleCount\":1627",
            "\"ttftSampleCount\":1500",
            "\"speedQuality\":\"generation\"",
        ] {
            assert!(json.contains(key), "缺少契约键 {key}: {json}");
        }
        // None 字段省略（旧缓存/旧载荷反序列化按缺省 None 兜底）
        let none_json = serde_json::to_string(&SpeedMetrics::default()).unwrap();
        assert_eq!(none_json, "{}", "{none_json}");
        let back: SpeedMetrics = serde_json::from_str(&none_json).unwrap();
        assert_eq!(back, SpeedMetrics::default());
    }

    /// 折叠基线（验收示例）：两个同名模型变体各 100 Token，生成时长分别
    /// 1s 与 0.1s（单行 100 / 1000 t/s）——真实合并均速 = 200×1000÷1100 ≈
    /// 181.8 t/s；按输出 Token 加权的错误算法会得到 550。Rust 侧保证字段
    /// 可加总（总体分子/分母 = Σ变体），TS 折叠（批次 2）按同公式求和后
    /// 相除即可得到与总体一致的均速。
    #[test]
    fn generation_speed_fold_baseline_181() {
        let conn = generation_db();
        // 变体一：100 tok / 1s → 100 t/s；变体二：100 tok / 0.1s → 1000 t/s
        insert_timed(
            &conn, 1_000, "GLM-5.3", 100, Some(1_100), Some(2_100), Some("completed"), None, None,
        );
        insert_timed(
            &conn, 3_000, "GLM-5.3", 100, Some(3_100), Some(3_200), Some("completed"), None, None,
        );
        // 两个 (provider, model) 分组、model 同名（模拟 foldModelStatRows 折叠）
        conn.execute_batch(
            "UPDATE model_usage SET provider_id = 'p1' WHERE started_at = 1000;
             UPDATE model_usage SET provider_id = 'p2' WHERE started_at = 3000;",
        )
        .unwrap();
        let (overall, by_model) = collect_generation_speed(&conn, 0, 100_000, 200_000).unwrap();
        assert_eq!(by_model.len(), 2);
        let mut sum_out = 0;
        let mut sum_ms = 0;
        for agg in by_model.values() {
            sum_out += agg.speed_output_tokens.unwrap();
            sum_ms += agg.speed_generation_ms.unwrap();
        }
        assert_eq!(sum_out, 200);
        assert_eq!(sum_ms, 1_100);
        // 总体（与折叠后同公式）：200×1000÷1100 ≈ 181.8，而不是 550
        assert!(approx(overall.avg_tps.unwrap(), 200_000.0 / 1_100.0));
        assert!((overall.avg_tps.unwrap() - 181.8).abs() < 0.1);
        assert_eq!(overall.speed_output_tokens, Some(sum_out));
        assert_eq!(overall.speed_generation_ms, Some(sum_ms));
        assert_eq!(overall.speed_sample_count, Some(2));
        assert!(approx(overall.max_tps.unwrap(), 1_000.0));
    }
}

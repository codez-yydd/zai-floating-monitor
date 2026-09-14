//! 模型速度统计面板数据源：按 model_id 分组聚合 ZCode 主库 model_usage
//! 的近段请求，输出每模型的输出速度分位、首 token 延迟、请求耗时与
//! token 量级（主面板"速度"tab 消费，`src/ModelSpeedPanel.tsx`）。
//!
//! 口径（速度参考 zcode-token-usage-statusbar：token/s = output_tokens ÷
//! (completed_at − first_token_at)，按请求时间戳而非 duration_ms 列）：
//! - 平均速度 = Σoutput ÷ Σ(completed_at − first_token_at)，仅统计
//!   status='completed' 且两时刻齐全的行；
//! - 慢/均/快速度 = 每请求速度（output × 1000 ÷ 生成毫秒）的
//!   P10/P50/P90 分位（行拉到 Rust 内存排序计算；近 7 天量级 < 1 万行）；
//! - 首 token 延迟 = first_token_at − started_at 的 min/avg/P90；
//! - 请求耗时 = completed_at − started_at 的 avg/P90；
//! - 输入/输出 token = 全部行 input_tokens（原值，已含缓存读）/
//!   output_tokens 的 avg/max；
//! - 成功率 = completed 行占比（status 列缺失的老库返回 null，前端
//!   显示 —）。
//!
//! 异常行防御：started_at/first_token_at/completed_at 任一为 NULL、
//! 非正或时序倒置（first < started、completed < first、completed <
//! started）的行只跳过**对应统计**，不影响其它字段；turn_id 为 NULL 的
//! CLI 后台请求行**不过滤**（速度统计对所有模型请求有效，与轮次无关）。
//!
//! 铁律：主库只读（zcode_sessions::open_main_db_readonly_uri），查询失败
//! 返回 Err 由前端展示；纯逻辑拆分为 collect_model_speed 供单元测试
//! （内存 sqlite 构造，不依赖真实 ~/.zcode）。

use rusqlite::Connection;
use serde::Serialize;
use std::collections::BTreeMap;

/// 单模型速度统计（Tauri command 返回；serde camelCase 与前端契约一致）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelSpeedStat {
    /// 模型 id（分组键）
    pub model_id: String,
    /// provider 简名（provider_id 取 "account:" 后部分，过长截断）
    pub provider: String,
    /// 请求数（窗口内 model_usage 行数）
    pub requests: i64,
    /// 成功率（completed 行占比，0–1；status 列缺失的老库为 null）
    pub success_rate: Option<f64>,
    /// 平均速度 t/s（Σoutput ÷ Σ生成毫秒；无可信行为 null）
    pub avg_tps: Option<f64>,
    /// 慢速（每请求速度 P10）
    pub p10_tps: Option<f64>,
    /// 均速（每请求速度 P50）
    pub p50_tps: Option<f64>,
    /// 快速（每请求速度 P90）
    pub p90_tps: Option<f64>,
    /// 首 token 延迟最小值（毫秒）
    pub ttft_min_ms: Option<i64>,
    /// 首 token 延迟均值（毫秒，四舍五入）
    pub ttft_avg_ms: Option<i64>,
    /// 首 token 延迟 P90（毫秒）
    pub ttft_p90_ms: Option<i64>,
    /// 请求耗时均值（completed_at − started_at，毫秒）
    pub dur_avg_ms: Option<i64>,
    /// 请求耗时 P90（毫秒）
    pub dur_p90_ms: Option<i64>,
    /// 输入 token 均值（input_tokens 原值，含缓存读）
    pub in_avg: i64,
    /// 输入 token 最大值
    pub in_max: i64,
    /// 输出 token 均值
    pub out_avg: i64,
    /// 输出 token 最大值
    pub out_max: i64,
}

/// 查询时间范围起点（毫秒，纯函数供测试）：
/// - days > 0：now − N×24h（滚动窗口）；
/// - days = 0（今日）：本地时区自然日零点（与主面板 today / 菜单栏
///   today_tray_title 的零点口径一致——不能用 UTC，否则东八区午前会把
///   昨天计入）。
fn range_start_ms(days: i64, now_ms: i64) -> i64 {
    if days > 0 {
        return now_ms - days * 86_400_000;
    }
    let Some(dt) = chrono::DateTime::from_timestamp_millis(now_ms) else {
        return now_ms - 86_400_000;
    };
    let local = dt.with_timezone(&chrono::Local);
    local
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|midnight| midnight.and_local_timezone(chrono::Local).single())
        .map(|d| d.timestamp_millis())
        .unwrap_or(now_ms - 86_400_000)
}

/// Tauri command：查询模型速度统计。days 缺省 7（近 7 天滚动）；0 = 今日
/// （本地零点起）。主库只读连接；表/核心列缺失（老版本库）返回空数组。
#[tauri::command]
pub fn get_model_speed(days: Option<i64>) -> Result<Vec<ModelSpeedStat>, String> {
    let days = days.unwrap_or(7).max(0);
    let now_ms = chrono::Utc::now().timestamp_millis();
    let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
    Ok(collect_model_speed(&conn, range_start_ms(days, now_ms))?)
}

/// 单模型聚合中间态（窗口内逐行累加，出口组装 DTO）
#[derive(Default)]
struct ModelAgg {
    requests: i64,
    completed: i64,
    provider_raw: Option<String>,
    /// 速度样本：(output_tokens, 生成毫秒)（completed 且时刻齐全时序正常）
    speed_pairs: Vec<(f64, f64)>,
    /// 首 token 延迟样本（毫秒）
    ttfts: Vec<i64>,
    /// 请求耗时样本（毫秒）
    durs: Vec<i64>,
    in_sum: i64,
    in_max: i64,
    out_sum: i64,
    out_max: i64,
}

/// 分位数（最近邻取整：idx = round(p/100 × (n−1))，升序切片）。空集
/// 返回 None。
fn percentile(sorted: &[f64], p: f64) -> Option<f64> {
    let n = sorted.len();
    if n == 0 {
        return None;
    }
    if n == 1 {
        return Some(sorted[0]);
    }
    let idx = ((p / 100.0) * (n as f64 - 1.0)).round() as usize;
    Some(sorted[idx.min(n - 1)])
}

/// i64 均值（四舍五入；空集 None）
fn avg_i64(vals: &[i64]) -> Option<i64> {
    if vals.is_empty() {
        return None;
    }
    let sum: i64 = vals.iter().sum();
    let n = vals.len() as i64;
    Some((sum + n / 2) / n)
}

/// provider_id 简名：取 "account:" 后部分（无前缀原样），超过 20 字符
/// 截断为前 18 + "…"（面板标签位有限，title 由前端给全量）
fn provider_short(raw: &str) -> String {
    let name = raw.split_once("account:").map(|(_, rest)| rest).unwrap_or(raw);
    if name.chars().count() > 20 {
        let head: String = name.chars().take(18).collect();
        format!("{head}…")
    } else {
        name.to_string()
    }
}

/// 纯聚合逻辑（不依赖真实 ~/.zcode，供单元测试）：读窗口内 model_usage
/// 行，按 model_id 分组组装统计。老版本库核心列缺失返回空数组（速度
/// 面板整体静默关闭）；非核心列缺失按 NULL 降级（对应统计为 null）。
pub(crate) fn collect_model_speed(
    conn: &Connection,
    from_ms: i64,
) -> Result<Vec<ModelSpeedStat>, String> {
    if !has_table(conn, "model_usage")
        || !crate::db::has_column(conn, "model_usage", "model_id")
        || !crate::db::has_column(conn, "model_usage", "started_at")
    {
        return Ok(Vec::new());
    }
    // 列探测降级（与 usage_feed/session_hud 同款防御模式）
    let opt = |col: &str| {
        if crate::db::has_column(conn, "model_usage", col) {
            col.to_string()
        } else {
            "NULL".to_string()
        }
    };
    let num = |col: &str| {
        if crate::db::has_column(conn, "model_usage", col) {
            format!("COALESCE({col}, 0)")
        } else {
            "0".to_string()
        }
    };
    let (provider, status, first, completed, inp, out) = (
        opt("provider_id"),
        opt("status"),
        opt("first_token_at"),
        opt("completed_at"),
        num("input_tokens"),
        num("output_tokens"),
    );
    let sql = format!(
        "SELECT COALESCE(model_id, ''), COALESCE({provider}, ''), {status}, \
                started_at, {first}, {completed}, {inp}, {out} \
         FROM model_usage \
         WHERE started_at >= ?1 AND model_id IS NOT NULL AND model_id != ''"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备模型速度查询失败: {e}"))?;
    let rows = stmt
        .query_map([from_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, Option<String>>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, Option<i64>>(5)?,
                row.get::<_, i64>(6)?,
                row.get::<_, i64>(7)?,
            ))
        })
        .map_err(|e| format!("读取模型速度行失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取模型速度行失败: {e}"))?;

    let mut groups: BTreeMap<String, ModelAgg> = BTreeMap::new();
    let has_status_col = status != "NULL";
    for (model, provider, status, started, first, completed, input, output) in rows {
        if model.is_empty() {
            continue;
        }
        let g = groups.entry(model).or_default();
        g.requests += 1;
        if g.provider_raw.is_none() && !provider.is_empty() {
            g.provider_raw = Some(provider);
        }
        if has_status_col && status.as_deref() == Some("completed") {
            g.completed += 1;
        } else if !has_status_col {
            // 老库无 status 列：model_usage 完成即落行，全部视作完成
            // （成功率无意义置 None，但 completed 计数仍用于一致性）
            g.completed += 1;
        }
        // 输入/输出 token：全部行参与（原值口径）
        g.in_sum += input.max(0);
        g.in_max = g.in_max.max(input);
        g.out_sum += output.max(0);
        g.out_max = g.out_max.max(output);
        // 首 token 延迟：first ≥ started 且两时刻有效（时序倒置/NULL 跳过）
        if let (Some(f), true) = (first, started > 0) {
            if f >= started {
                g.ttfts.push(f - started);
            }
        }
        // 请求耗时：completed > started 且两时刻有效
        if let (Some(c), true) = (completed, started > 0) {
            if c > started {
                g.durs.push(c - started);
            }
        }
        // 速度样本：completed 状态行且 completed > first > 0（时序齐全；
        // 状态列缺失的老库按全完成降级参与——速度统计本身不依赖轮状态）
        let is_completed = !has_status_col || status.as_deref() == Some("completed");
        if is_completed && output > 0 {
            if let (Some(c), Some(f)) = (completed, first) {
                if c > f && f > 0 {
                    g.speed_pairs.push((output as f64, (c - f) as f64));
                }
            }
        }
    }

    let mut out: Vec<ModelSpeedStat> = Vec::with_capacity(groups.len());
    for (model, g) in groups {
        // 速度：均值 = Σout ÷ Σ生成毫秒 × 1000；分位 = 每请求速度
        let (avg_tps, p10, p50, p90) = if g.speed_pairs.is_empty() {
            (None, None, None, None)
        } else {
            let sum_out: f64 = g.speed_pairs.iter().map(|(o, _)| *o).sum();
            let sum_ms: f64 = g.speed_pairs.iter().map(|(_, m)| *m).sum();
            let avg = if sum_ms > 0.0 {
                Some(sum_out * 1000.0 / sum_ms)
            } else {
                None
            };
            let mut speeds: Vec<f64> = g
                .speed_pairs
                .iter()
                .map(|(o, m)| o * 1000.0 / *m)
                .collect();
            speeds.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
            (
                avg,
                percentile(&speeds, 10.0),
                percentile(&speeds, 50.0),
                percentile(&speeds, 90.0),
            )
        };
        // 首 token / 耗时分位（毫秒，升序后取分位再截断）
        let mut ttfts = g.ttfts.clone();
        ttfts.sort_unstable();
        let mut durs = g.durs.clone();
        durs.sort_unstable();
        let ttft_f64: Vec<f64> = ttfts.iter().map(|&v| v as f64).collect();
        let dur_f64: Vec<f64> = durs.iter().map(|&v| v as f64).collect();
        let success_rate = if has_status_col && g.requests > 0 {
            Some(g.completed as f64 / g.requests as f64)
        } else {
            None
        };
        let req_n = g.requests.max(1);
        out.push(ModelSpeedStat {
            model_id: model,
            provider: provider_raw_short(&g.provider_raw),
            requests: g.requests,
            success_rate,
            avg_tps,
            p10_tps: p10,
            p50_tps: p50,
            p90_tps: p90,
            ttft_min_ms: ttfts.first().copied(),
            ttft_avg_ms: avg_i64(&ttfts),
            ttft_p90_ms: percentile(&ttft_f64, 90.0).map(|v| v as i64),
            dur_avg_ms: avg_i64(&durs),
            dur_p90_ms: percentile(&dur_f64, 90.0).map(|v| v as i64),
            in_avg: (g.in_sum + req_n / 2) / req_n,
            in_max: g.in_max,
            out_avg: (g.out_sum + req_n / 2) / req_n,
            out_max: g.out_max,
        });
    }
    // 请求数降序（高频模型在前），同数按模型名稳定排序
    out.sort_by(|a, b| b.requests.cmp(&a.requests).then_with(|| a.model_id.cmp(&b.model_id)));
    Ok(out)
}

/// provider 原始值 → 简名（缺失时空串）
fn provider_raw_short(raw: &Option<String>) -> String {
    raw.as_deref().map(provider_short).unwrap_or_default()
}

/// 表存在性探测（模块自持一份，与 usage_feed/session_hud 同款实现）
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 速度统计测试库（model_usage 按实测 schema 最小化）
    fn speed_db(name: &str) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                first_token_at INTEGER, completed_at INTEGER, model_id TEXT,
                provider_id TEXT, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn 分位数_最近邻与边界() {
        assert_eq!(percentile(&[], 50.0), None);
        assert_eq!(percentile(&[7.0], 10.0), Some(7.0));
        // 4 样本 [10,20,30,40]：P10 → round(0.1×3)=0 → 10；P50 →
        // round(1.5)=2 → 30；P90 → round(2.7)=3 → 40
        let v = [10.0, 20.0, 30.0, 40.0];
        assert_eq!(percentile(&v, 10.0), Some(10.0));
        assert_eq!(percentile(&v, 50.0), Some(30.0));
        assert_eq!(percentile(&v, 90.0), Some(40.0));
        assert_eq!(percentile(&v, 0.0), Some(10.0));
        assert_eq!(percentile(&v, 100.0), Some(40.0));
        // 五分位经典值：[1..=5] 的 P50 = 3
        assert_eq!(percentile(&[1.0, 2.0, 3.0, 4.0, 5.0], 50.0), Some(3.0));
    }

    #[test]
    fn 均值_四舍五入() {
        assert_eq!(avg_i64(&[]), None);
        assert_eq!(avg_i64(&[10]), Some(10));
        assert_eq!(avg_i64(&[10, 15]), Some(13), "12.5 四舍五入 13");
        assert_eq!(avg_i64(&[-10, -15]), Some(-12), "-12.5 向上取整 -12");
    }

    #[test]
    fn provider简名_account前缀剥离与截断() {
        assert_eq!(provider_short("account:bigmodel-individual"), "bigmodel-individual");
        assert_eq!(provider_short("doubao"), "doubao");
        // 21 字符截断为 18 + "…"
        let long = "a".repeat(21);
        let short = provider_short(&long);
        assert_eq!(short.chars().count(), 19);
        assert!(short.ends_with('…'));
        // 恰 20 字符不截断
        assert_eq!(provider_short(&"b".repeat(20)), "b".repeat(20));
    }

    #[test]
    fn 聚合_速度分位与token量级与成功率() {
        let (conn, path) = speed_db("agg");
        let t0 = 1_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- A 模型：两笔可信速度样本（1000ms 生成 100tok → 100 t/s；
              -- 500ms 生成 50tok → 100 t/s）+ 一笔 error 行（计入请求与
              -- token，不计速度）
              ('s1', 't1', {t0}, {f1}, {c1}, 'GLM-5.3', 'account:bigmodel-individual', 'completed', 1000, 100),
              ('s1', 't2', {t2}, {f2}, {c2}, 'GLM-5.3', 'account:bigmodel-individual', 'completed', 800, 50),
              ('s1', 't3', {t3}, {f3}, {c3}, 'GLM-5.3', 'account:bigmodel-individual', 'error', 600, 0),
              -- B 模型：窗口外行（不参与）
              ('s1', 't9', {old}, {of}, {oc}, 'GLM-5.3', 'account:x', 'completed', 999, 999),
              -- C 模型：turn_id NULL 的后台请求（不过滤：计入全部统计）
              ('s1', NULL, {t4}, {f4}, {c4}, 'GLM-4.7', 'doubao', 'completed', 200, 30);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
            t2 = t0 + 10_000,
            f2 = t0 + 10_200,
            c2 = t0 + 10_700,
            t3 = t0 + 20_000,
            f3 = t0 + 20_100,
            c3 = t0 + 20_200,
            old = t0 - 100_000,
            of = t0 - 99_000,
            oc = t0 - 98_000,
            t4 = t0 + 30_000,
            f4 = t0 + 30_100,
            c4 = t0 + 30_500,
        ))
        .unwrap();
        let stats = collect_model_speed(&conn, t0).unwrap();
        assert_eq!(stats.len(), 2, "{stats:?}");
        // 排序：A（3 请求）在前，C（1 请求）在后
        let a = &stats[0];
        assert_eq!(a.model_id, "GLM-5.3");
        assert_eq!(a.provider, "bigmodel-individual", "provider 应取 account: 后简名");
        assert_eq!(a.requests, 3);
        // 成功率 2/3
        assert!((a.success_rate.unwrap() - 2.0 / 3.0).abs() < 1e-9);
        // 平均速度 = (100+50)×1000 ÷ (1000+500) = 100.0；P10/P50/P90 均 100
        assert!((a.avg_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p10_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p50_tps.unwrap() - 100.0).abs() < 1e-6);
        assert!((a.p90_tps.unwrap() - 100.0).abs() < 1e-6);
        // TTFT：500/200/100 → min 100、avg 267、P90 500
        assert_eq!(a.ttft_min_ms, Some(100));
        assert_eq!(a.ttft_avg_ms, Some(267));
        assert_eq!(a.ttft_p90_ms, Some(500));
        // 耗时（completed−started，不分状态——error 行的耗时同样有效）：
        // 1500/700/200 → avg 800、P90 1500
        assert_eq!(a.dur_avg_ms, Some(800));
        assert_eq!(a.dur_p90_ms, Some(1500));
        // token：in avg = (1000+800+600+0)/3 → 800（error 行 input 600 计入）
        assert_eq!(a.in_avg, 800);
        assert_eq!(a.in_max, 1000);
        assert_eq!(a.out_avg, 50);
        assert_eq!(a.out_max, 100);
        // C 模型（NULL turn_id 后台请求不过滤）：TTFT 100、生成 400ms
        // 30 tok → 75 t/s
        let c = &stats[1];
        assert_eq!(c.model_id, "GLM-4.7");
        assert_eq!(c.provider, "doubao");
        assert_eq!(c.requests, 1);
        assert!((c.avg_tps.unwrap() - 75.0).abs() < 1e-6);
        assert_eq!(c.ttft_min_ms, Some(100));
        drop(conn);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn 聚合_异常行防御与降级() {
        let (conn, path) = speed_db("dirty");
        let t0 = 2_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, first_token_at,
                completed_at, model_id, provider_id, status, input_tokens, output_tokens) VALUES
              -- 时刻 NULL 行：跳过速度/TTFT/耗时，计入请求数与 token
              ('s1', 't1', {t0}, NULL, {c1}, 'M1', 'account:p', 'completed', 100, 10),
              -- 时序倒置（completed < first）：跳过速度；耗时（completed>started）仍有效
              ('s1', 't2', {t0}, {f2}, {c2}, 'M1', 'account:p', 'completed', 100, 10),
              -- output=0 的 completed 行：不产生速度样本（0 速度无意义），
              -- TTFT/耗时正常计入
              ('s1', 't3', {t0}, {f3}, {c3}, 'M1', 'account:p', 'completed', 100, 0);",
            t0 = t0,
            c1 = t0 + 900,
            f2 = t0 + 800,
            c2 = t0 + 500,
            f3 = t0 + 100,
            c3 = t0 + 300,
        ))
        .unwrap();
        let stats = collect_model_speed(&conn, t0).unwrap();
        assert_eq!(stats.len(), 1);
        let m = &stats[0];
        assert_eq!(m.requests, 3);
        assert_eq!(m.success_rate, Some(1.0));
        assert_eq!(m.avg_tps, None, "全部速度样本不可信应为 null");
        assert_eq!(m.p50_tps, None);
        // TTFT：t2 行 first(800) ≥ started 但 completed<first 不影响 TTFT；
        // 样本 = [800, 100] → min 100、P90 800
        assert_eq!(m.ttft_min_ms, Some(100));
        assert_eq!(m.ttft_p90_ms, Some(800));
        // 耗时：t1 行 900（first NULL 不影响耗时）、t2 行 completed(t0+500)
        // > started 有效 500、t3 行 300 → 样本 [900, 500, 300] → avg 567、
        // P90 900（P90 最近邻取 idx=round(0.9×2)=2）
        assert_eq!(m.dur_avg_ms, Some(567));
        assert_eq!(m.dur_p90_ms, Some(900));
        drop(conn);
        let _ = std::fs::remove_file(&path);

        // 老版本库缺 status 列：成功率 null（无状态可判），速度按全完成
        // 降级参与
        let path2 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-nostatus.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path2);
        let conn = Connection::open(&path2).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, first_token_at INTEGER, completed_at INTEGER,
                model_id TEXT, provider_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ({t0}, {f1}, {c1}, 'M1', 'account:p', 100, 100);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
        ))
        .unwrap();
        let stats = collect_model_speed(&conn, t0).unwrap();
        assert_eq!(stats.len(), 1);
        assert_eq!(stats[0].success_rate, None, "缺 status 列成功率应为 null");
        assert!((stats[0].avg_tps.unwrap() - 100.0).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_file(&path2);

        // 更老版本库缺 provider_id 列：整体不报错，其余统计正常，
        // 仅 provider 展示为空
        let path4 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-noprovider.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path4);
        let conn = Connection::open(&path4).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                started_at INTEGER, first_token_at INTEGER, completed_at INTEGER,
                model_id TEXT, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ({t0}, {f1}, {c1}, 'M1', 'completed', 100, 100);",
            t0 = t0,
            f1 = t0 + 500,
            c1 = t0 + 1500,
        ))
        .unwrap();
        let stats = collect_model_speed(&conn, t0).unwrap();
        assert_eq!(stats.len(), 1, "缺 provider_id 列不应整体失败");
        assert_eq!(stats[0].provider, "", "缺 provider_id 列 provider 应为空");
        assert_eq!(stats[0].requests, 1);
        assert_eq!(stats[0].success_rate, Some(1.0));
        assert!((stats[0].avg_tps.unwrap() - 100.0).abs() < 1e-6);
        drop(conn);
        let _ = std::fs::remove_file(&path4);

        // 无 model_usage 表（极端老库）→ 空数组不报错
        let path3 = std::env::temp_dir().join(format!(
            "zbar-model-speed-db-{}-empty.sqlite",
            std::process::id()
        ));
        let _ = std::fs::remove_file(&path3);
        let conn = Connection::open(&path3).unwrap();
        assert!(collect_model_speed(&conn, 0).unwrap().is_empty());
        drop(conn);
        let _ = std::fs::remove_file(&path3);
    }

    #[test]
    fn 时间范围_今日零点与滚动窗口() {
        use chrono::Timelike as _;
        // 滚动窗口：now − N 天
        let now = 5_000_000_000_000_i64;
        assert_eq!(range_start_ms(7, now), now - 7 * 86_400_000);
        assert_eq!(range_start_ms(1, now), now - 86_400_000);
        // 今日：本地零点（结果落在 [now−24h, now] 内，且为本地零点整点）
        let start = range_start_ms(0, now);
        assert!(start <= now && start > now - 86_400_000, "{start}");
        let dt = chrono::DateTime::from_timestamp_millis(start).unwrap();
        let local = dt.with_timezone(&chrono::Local);
        assert_eq!(local.time().second(), 0);
        assert_eq!(local.time().minute(), 0);
        assert_eq!(local.time().hour(), 0);
        // 负数防御按 0 处理
        assert_eq!(range_start_ms(-3, now), range_start_ms(0, now));
    }
}

//! 额度快照历史：append-only JSONL 存储 + 周期解析查询。
//!
//! 设计要点：
//! - 快照来源：每次 quota.rs::fetch_quota 成功后追加写一条。
//! - 存储：~/.zbar/quota_history.jsonl（每行一条 JSON），轻量、可 append、易调试。
//! - 周期划分：用 weekly_reset (nextResetTime) 的变化点切分"智谱重置周期"。
//! - 去重/限频：同秒内只写一条（用最后一条的 ts 防抖）。

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

use crate::pricing::config_dir;

/// 单条额度快照（jsonl 一行）。
/// 字段尽量与 QuotaResult 对齐，只挑有价值的几项，控制单条体积。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaSnapshot {
    /// 采样毫秒时间戳（UTC，与 model_usage.started_at 口径一致）
    pub ts: i64,
    /// 套餐等级："pro" / "max" ...
    #[serde(default)]
    pub level: String,
    /// weekly 已用百分比 0-100
    #[serde(default)]
    pub weekly_pct: u32,
    /// weekly 下次重置时间（毫秒）；周期边界的关键字段
    #[serde(default)]
    pub weekly_reset: Option<i64>,
    /// 5 小时窗口已用百分比（顺带记录，零成本）
    #[serde(default)]
    pub hour5_pct: u32,
    /// MCP 月度已用百分比（顺带）
    #[serde(default)]
    pub mcp_pct: u32,
    /// MCP 已用次数（如有）
    #[serde(default)]
    pub mcp_used: Option<i64>,
    /// MCP 总额度次数（如有）
    #[serde(default)]
    pub mcp_total: Option<i64>,
}

/// ~/.zbar/quota_history.jsonl
pub fn history_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("quota_history.jsonl"))
}

/// 追加一条快照。同秒内只写一条（防抖）。
/// 写失败不影响调用方（额度查询本身），因此静默降级。
pub fn append_snapshot(snap: &QuotaSnapshot) {
    if let Err(e) = try_append(snap) {
        eprintln!("[zbar-history] 写快照失败: {e}");
    }
}

fn try_append(snap: &QuotaSnapshot) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = history_path()?;

    // 防抖：读最后一条，若同秒则覆盖最后一条而非新增。
    if path.exists() {
        if let Ok(last_ts) = read_last_ts(&path) {
            // 同一条的秒级时间戳相同 → 视为同一次刷新的重复采样，跳过
            if last_ts / 1000 == snap.ts / 1000 {
                return Ok(());
            }
        }
    }

    let line = serde_json::to_string(snap)
        .map_err(|e| format!("序列化快照失败: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    // 每行一条，末尾换行
    writeln!(file, "{line}").map_err(|e| format!("写入快照失败: {e}"))
}

/// 读取最后一条快照的 ts（用于防抖判断）。
/// 采用从文件末尾回扫的方式，避免全文件读取。
fn read_last_ts(path: &PathBuf) -> Result<i64, String> {
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let mut reader = BufReader::new(file);
    let mut last_line = String::new();
    let mut buf = String::new();
    // 逐行读，保留最后非空行
    while reader
        .read_line(&mut buf)
        .map_err(|e| format!("读取快照失败: {e}"))?
        > 0
    {
        if !buf.trim().is_empty() {
            last_line = buf.trim().to_string();
        }
        buf.clear();
    }
    if last_line.is_empty() {
        return Ok(0);
    }
    let v: QuotaSnapshot = serde_json::from_str(&last_line)
        .map_err(|e| format!("解析最后一条快照失败: {e}"))?;
    Ok(v.ts)
}

/// 读取全部快照（按 ts 升序）。损坏的行跳过，不整体失败。
pub fn load_all() -> Result<Vec<QuotaSnapshot>, String> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue,
        };
        if line.trim().is_empty() {
            continue;
        }
        if let Ok(snap) = serde_json::from_str::<QuotaSnapshot>(&line) {
            out.push(snap);
        }
    }
    // 保证升序
    out.sort_by_key(|s| s.ts);
    Ok(out)
}

/// 清空历史（设置页"清理历史"用）。
pub fn clear_history() -> Result<(), String> {
    let path = history_path()?;
    if path.exists() {
        fs::remove_file(&path).map_err(|e| format!("删除快照文件失败: {e}"))?;
    }
    Ok(())
}

// ===== 周期解析 =====

/// 一个"智谱重置周期"的汇总（供对比页直接渲染）。
#[derive(Debug, Clone, Serialize)]
pub struct WeeklyPeriod {
    /// 周期开始（重置时刻）。首条历史周期可能拿不到，用首条快照 ts 兜底。
    pub reset_at: i64,
    /// 周期结束（下一次重置时刻）。当前未结束周期用 now 兜底。
    pub end_at: i64,
    /// 是否当前未结束的周期
    pub is_current: bool,
    /// 周期内 weekly 起始百分比（第一条快照）
    pub pct_start: u32,
    /// 周期内 weekly 峰值百分比
    pub pct_peak: u32,
    /// 周期内 weekly 结束百分比（最后一条快照）
    pub pct_end: u32,
    /// 周期内采样数（衡量可信度）
    pub sample_count: u32,
}

/// 把快照序列按 weekly_reset 的跳变点切分成多个周期。
///
/// 核心语义（关键，勿混淆）：
/// - 每条快照的 weekly_reset = "此刻的下次重置时间"（未来的）
/// - 当 weekly_reset 发生跳变（新值 > 旧值）时，说明刚发生了一次重置，
///   重置时刻 ≈ 跳变发生处的快照 ts。这个跳变点就是前后两周期的分界。
/// - 周期[i] 的 reset_at(开始) = 跳变点的 ts（重置发生时刻）
/// - 周期[i] 的 end_at(结束) = 周期内快照的 weekly_reset（下次重置时刻）
/// - 兼容补差价/重新订阅导致的提前重置：任何大幅跳变都视为新周期。
pub fn split_periods(snaps: &[QuotaSnapshot]) -> Vec<WeeklyPeriod> {
    if snaps.is_empty() {
        return Vec::new();
    }

    let now_ms = chrono::Local::now().timestamp_millis();
    let mut periods: Vec<WeeklyPeriod> = Vec::new();

    // 当前正在累积的周期的快照索引区间 [start_idx, end_idx)
    let mut start_idx = 0usize;
    // 当前周期的开始时间（重置时刻）
    // 第一条无法知道真正的重置时刻 → 用首条快照 ts 兜底（可能比真实开始晚）
    let mut cur_start = snaps[0].ts;

    for i in 1..snaps.len() {
        let prev_reset = snaps[i - 1].weekly_reset;
        let cur_reset = snaps[i].weekly_reset;
        // 跳变检测：前一条的 weekly_reset 是某未来时间 T，
        // 当前条的 weekly_reset 明显更大（>= 1天），说明 T 已经过去并发生了重置，
        // 新的 nextResetTime 变成了 T'（更远）。这个跳变 = 重置发生。
        let jumped = match (prev_reset, cur_reset) {
            (Some(p), Some(c)) => c > p + 86_400_000,
            _ => false,
        };
        if jumped {
            // [start_idx, i) 这些快照属于上一周期（跳变前的）
            // 上一周期的 end = 它们共同的 weekly_reset（= prev_reset）
            let prev_end = snaps[i - 1].weekly_reset.unwrap_or(snaps[i - 1].ts);
            periods.push(build_period(
                &snaps[start_idx..i],
                cur_start,
                prev_end,
                false,
                now_ms,
            ));
            // 新周期从这里开始，重置时刻 ≈ 当前快照 ts
            start_idx = i;
            cur_start = snaps[i].ts;
        }
    }
    // 收尾最后一个周期（当前未结束）
    let last = &snaps[snaps.len() - 1];
    periods.push(build_period(
        &snaps[start_idx..],
        cur_start,
        last.weekly_reset.unwrap_or(last.ts),
        true,
        now_ms,
    ));
    periods
}

/// 用一组快照构建一个周期汇总。
/// - reset_at: 周期开始（重置时刻，可能为首条 ts 兜底）
/// - next_reset: 周期结束（下次重置时刻；当前周期用 now）
fn build_period(
    snaps: &[QuotaSnapshot],
    reset_at: i64,
    next_reset: i64,
    is_current: bool,
    now_ms: i64,
) -> WeeklyPeriod {
    let pct_start = snaps.first().map(|s| s.weekly_pct).unwrap_or(0);
    let pct_end = snaps.last().map(|s| s.weekly_pct).unwrap_or(0);
    let pct_peak = snaps.iter().map(|s| s.weekly_pct).max().unwrap_or(0);
    // 当前周期的 end 用 now（实时延伸）；已结束周期用 next_reset
    let end_at = if is_current { now_ms } else { next_reset };
    WeeklyPeriod {
        reset_at,
        end_at,
        is_current,
        pct_start,
        pct_peak,
        pct_end,
        sample_count: snaps.len() as u32,
    }
}

/// 取"今日"快照（本地 0 点之后）。
pub fn today_snapshots() -> Result<Vec<QuotaSnapshot>, String> {
    let all = load_all()?;
    let today_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or(0);
    Ok(all.into_iter().filter(|s| s.ts >= today_start).collect())
}

/// 今日增量 = 今日峰值百分比 - 今日起始百分比（纯前端也可算，这里提供一份）。
/// 返回 (增量, 今日采样数)。
pub fn today_delta() -> Result<(u32, u32), String> {
    let today = today_snapshots()?;
    if today.len() < 2 {
        return Ok((0, today.len() as u32));
    }
    let start = today.first().map(|s| s.weekly_pct).unwrap_or(0);
    let peak = today.iter().map(|s| s.weekly_pct).max().unwrap_or(0);
    // peak 一定 >= start；增量用峰值减起点，反映当日真实消耗
    Ok((peak.saturating_sub(start), today.len() as u32))
}

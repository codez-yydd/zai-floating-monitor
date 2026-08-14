//! 额度快照历史：append-only JSONL 存储 + 周期解析查询。
//!
//! 设计要点：
//! - 快照来源：每次 quota.rs::fetch_quota 成功后追加写一条。
//! - 存储：~/.zbar/quota_history.jsonl（每行一条 JSON），轻量、可 append、易调试。
//! - 周期划分：用 weekly_reset (nextResetTime) 的变化点切分"智谱重置周期"。
//! - 去重/限频：同秒内只写一条（用最后一条的 ts 防抖）。
//! - 滚动保留：定期（每 24h 至多一次）删除超过保留期的行，防止文件无限增长。

use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::pricing::config_dir;

/// 快照保留期：超过 90 天的行在滚动清理时删除（约 430KB/天，不清理会无限增长）
const RETENTION_MS: i64 = 90 * 86_400_000;
/// 清理检查最小间隔：避免每次写入都触发全量重写（读写整个文件）
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);
/// 上次清理时间（None = 本进程尚未清理过）。
/// 模块级 Mutex 节流：快照写入来自多个线程（fetch_quota / 同步 worker）。
static LAST_CLEANUP: OnceLock<Mutex<Option<SystemTime>>> = OnceLock::new();

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

    // 滚动清理：写入前检查（内部有 24h 节流，不会每次写都全量重写）
    maybe_cleanup(&path);

    // 防抖：读最后一条，若同秒则覆盖最后一条而非新增。
    if path.exists() {
        if let Ok(last_ts) = read_last_ts(&path) {
            // 同一条的秒级时间戳相同 → 视为同一次刷新的重复采样，跳过
            if last_ts / 1000 == snap.ts / 1000 {
                return Ok(());
            }
        }
        // 防残行拼接：若上次进程崩溃残留了无换行的半截行，直接 append 会
        // 与残行拼成一行导致 JSON 损坏（load_all 会跳过整行丢新快照）。
        // 文件尾字节非 \n 时先补一个换行，让新快照独占一行。
        ensure_trailing_newline(&path)?;
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

/// 确保文件以换行符结尾：尾部是半截残行（进程崩溃残留）时补一个 \n，
/// 避免后续 append 的快照与残行拼成一行。空文件视为无需处理。
fn ensure_trailing_newline(path: &std::path::Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("定位快照文件失败: {e}"))?;
    if len == 0 {
        return Ok(());
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))
        .map_err(|e| format!("定位快照文件失败: {e}"))?;
    file.read_exact(&mut last)
        .map_err(|e| format!("读取快照文件末字节失败: {e}"))?;
    if last[0] != b'\n' {
        file.seek(SeekFrom::End(0))
            .map_err(|e| format!("定位快照文件失败: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("补写换行失败: {e}"))?;
    }
    Ok(())
}

/// 滚动清理入口：删除超过保留期的快照行。
/// 用 LAST_CLEANUP 节流为每 24h 至多执行一次；失败时也推进节流时钟
/// （最坏情况文件多保留一天，避免持续失败时每 30s 全量重写）。
fn maybe_cleanup(path: &PathBuf) {
    let cell = LAST_CLEANUP.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = cell.lock() else {
        return; // 锁中毒等异常：跳过清理，不影响写入主流程
    };
    if let Some(t) = *last {
        if t.elapsed().unwrap_or(Duration::ZERO) < CLEANUP_INTERVAL {
            return; // 距上次清理不足 24h，跳过
        }
    }
    // 无论本次是否删出内容都推进时钟，之后再释放锁（重写文件耗时不应阻塞写入方）
    *last = Some(SystemTime::now());
    drop(last);

    if let Err(e) = try_cleanup(path) {
        eprintln!("[zbar-history] 滚动清理失败（24h 后重试）: {e}");
    }
}

/// 清理实现：过滤掉 ts 早于保留边界的行，写临时文件后 rename 原子替换。
/// 损坏的行原样保留（不丢数据，与 load_all 的跳过策略对齐）。
///
/// 已知取舍：rename 与并发 try_append 之间存在毫秒级窗口——若恰在此刻有写入
/// 落到旧 inode，rename 后那一条快照会丢失（文件本身不会损坏）。考虑到清理
/// 每 24h 才执行一次、丢的至多 1 条非关键采样（30s 后就会补采），接受现状，
/// 不为此引入全局写锁。
fn try_cleanup(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = chrono::Local::now().timestamp_millis() - RETENTION_MS;

    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let reader = BufReader::new(file);

    let mut kept: Vec<String> = Vec::new();
    let mut removed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(_) => continue, // 读失败的行无法判断，直接丢弃（与 load_all 跳过策略一致）
        };
        if line.trim().is_empty() {
            continue;
        }
        // 只取 ts 字段判断是否过期，避免严格反序列化其他字段格式变化导致误删
        let ts = serde_json::from_str::<serde_json::Value>(&line)
            .ok()
            .and_then(|v| v.get("ts").and_then(|t| t.as_i64()));
        match ts {
            Some(t) if t < cutoff => removed += 1,
            _ => kept.push(line), // 无 ts 的损坏行按保留处理，不丢数据
        }
    }
    if removed == 0 {
        return Ok(()); // 无过期行，不必重写
    }

    // 临时文件与目标同目录，rename 在同一文件系统上保证原子性
    let tmp = path.with_extension("jsonl.tmp");
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("打开临时文件失败: {e}"))?;
    for line in &kept {
        writeln!(out, "{line}").map_err(|e| format!("写入临时文件失败: {e}"))?;
    }
    out.flush().map_err(|e| format!("刷新临时文件失败: {e}"))?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| format!("替换快照文件失败: {e}"))
}

/// 读取最后一条快照的 ts（用于防抖判断）。
/// 从文件末尾往回读一个小窗口，在窗口内找最后一个完整 JSON 行解析，
/// 避免全文件读取。单行快照 JSON 可能超过初始窗口，不够时逐倍扩大（上限 64KB）。
fn read_last_ts(path: &PathBuf) -> Result<i64, String> {
    let mut file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开快照文件失败: {e}"))?;
    let len = file
        .metadata()
        .map_err(|e| format!("读取快照文件元数据失败: {e}"))?
        .len();
    if len == 0 {
        return Ok(0);
    }

    let mut window: u64 = 2048;
    const MAX_WINDOW: u64 = 64 * 1024;
    loop {
        let start = len.saturating_sub(window);
        file.seek(SeekFrom::Start(start))
            .map_err(|e| format!("定位快照文件失败: {e}"))?;
        let mut buf = Vec::with_capacity((len - start) as usize);
        file.read_to_end(&mut buf)
            .map_err(|e| format!("读取快照失败: {e}"))?;
        let text = String::from_utf8_lossy(&buf);

        // 窗口首行可能被左边界截断（start > 0 时），不可信，跳过；
        // 其余行前面必有换行边界，均为完整行。
        let mut last_line: Option<&str> = None;
        for (i, line) in text.lines().enumerate() {
            if start > 0 && i == 0 {
                continue;
            }
            if !line.trim().is_empty() {
                last_line = Some(line);
            }
        }

        match last_line {
            Some(line) => {
                return serde_json::from_str::<QuotaSnapshot>(line.trim())
                    .map(|v| v.ts)
                    .map_err(|e| format!("解析最后一条快照失败: {e}"));
            }
            None => {
                // 窗口内没找到完整非空行：已读全文件则确实没有；否则扩大窗口重试
                if start > 0 && window < MAX_WINDOW {
                    window *= 2;
                    continue;
                }
                return Ok(0);
            }
        }
    }
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
/// - 当 weekly_reset 发生跳变（新值 > 旧值）时，说明刚发生了一次重置。
///   这个跳变点就是前后两周期的分界。
/// - 周期[i] 的 reset_at(开始) = min(跳变前最后一条快照预告的 weekly_reset,
///   跳变后首条快照 ts)：常规错过跳变场景取服务端预告的准确重置时刻；
///   服务端提前重置（旧预告时刻未到就跳变）时取快照 ts，两种场景都不漏用量
/// - 周期[i] 的 end_at(结束) = 与下一周期 reset_at 共用的统一分界
///   min(周期内最后一条快照预告的 weekly_reset, 下一周期首条快照 ts)，
///   保证相邻周期区间无缝不重叠（SQL 逐周期独立聚合，重叠会双计）
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
            // 统一分界 = min(跳变前快照预告的 weekly_reset, 跳变后首条快照 ts)。
            // 上一周期的 end_at 与新周期的 reset_at 必须共用同一分界，区间才无缝
            // 不重叠——本地 SQL 与前端远端归属均按 [reset_at, end_at) 逐周期独立
            // 聚合，两个分界不一致会让重叠段的用量双计进两个周期：
            // - 常规场景（应用在重置后才恢复采样，snaps[i].ts > 预告时刻）：分界 =
            //   预告的准确重置时刻，闭合"重置~首条快照"的漏算缺口（否则对比页
            //   "实际 Token"偏低）；
            // - 服务端提前重置场景（跳变发生时旧预告时刻还没到，snaps[i].ts < 预告
            //   时刻）：分界 = snaps[i].ts，避免把 [snaps[i].ts, 预告时刻) 的用量
            //   漏出所有周期。
            // i 从 1 起循环，i-1 恒存在；jumped 要求 prev_reset 为 Some，unwrap_or 仅为兜底。
            let boundary = prev_reset.unwrap_or(snaps[i].ts).min(snaps[i].ts);
            // [start_idx, i) 这些快照属于上一周期（跳变前的）
            periods.push(build_period(
                &snaps[start_idx..i],
                cur_start,
                boundary,
                false,
                now_ms,
            ));
            start_idx = i;
            cur_start = boundary;
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

#[cfg(test)]
mod tests {
    use super::*;

    /// 生成临时测试文件路径（用例各自创建/清理，不引入 tempfile 依赖）
    fn temp_jsonl(name: &str) -> PathBuf {
        let dir = std::env::temp_dir();
        std::fs::create_dir_all(&dir).ok();
        dir.join(format!(
            "zbar-history-test-{name}-{}.jsonl",
            std::process::id()
        ))
    }

    fn snap(ts: i64, level: &str) -> QuotaSnapshot {
        QuotaSnapshot {
            ts,
            level: level.to_string(),
            weekly_pct: 10,
            weekly_reset: Some(ts + 86_400_000),
            hour5_pct: 5,
            mcp_pct: 0,
            mcp_used: None,
            mcp_total: None,
        }
    }

    /// 生成指定 weekly_reset 的快照（周期切分用例需要精确控制预告重置时刻）
    fn snap_at(ts: i64, weekly_reset: Option<i64>) -> QuotaSnapshot {
        QuotaSnapshot {
            weekly_reset,
            ..snap(ts, "pro")
        }
    }

    /// 周期切分：跳变前快照预告的 weekly_reset 落在跳变后快照 ts 之前数小时
    /// （应用重置后很久才启动的场景），新周期 reset_at 应等于预告重置时刻，
    /// 否则真实重置~首条快照之间的用量会漏出所有周期
    #[test]
    fn split_periods_uses_announced_reset_as_new_start() {
        // 周五 22:00 采样，预告周六 02:00 重置；应用关机，周一 20:00 才恢复采样
        let announced = 1_800_000_000_000_i64;
        let before_ts = announced - 4 * 3600_000;
        let before = snap_at(before_ts, Some(announced));
        let after = snap_at(
            announced + 66 * 3600_000,
            Some(announced + 7 * 86_400_000),
        );
        let periods = split_periods(&[before, after]);
        assert_eq!(periods.len(), 2, "weekly_reset 大幅跳变应切出两个周期");
        // 上一周期：起点用首条快照 ts 兜底，终点 = 预告重置时刻
        assert_eq!(periods[0].reset_at, before_ts);
        assert_eq!(periods[0].end_at, announced, "上一周期终点应为预告重置时刻");
        // 新周期：起点 = 预告重置时刻（而非首条快照 ts），闭合 66h 的漏算缺口
        assert_eq!(periods[1].reset_at, announced);
        assert!(periods[0].end_at <= periods[1].reset_at, "相邻周期必须无缝不重叠");
        assert!(periods[1].is_current);
    }

    /// 周期切分：服务端提前重置（跳变发生时旧预告时刻还没到，snaps[i].ts <
    /// 预告时刻）新周期起点应取 snaps[i].ts，避免 [snaps[i].ts, 预告时刻)
    /// 的用量漏出所有周期
    #[test]
    fn split_periods_early_reset_uses_snapshot_ts() {
        let base = 1_800_000_000_000_i64;
        // 采样时预告 20 小时后重置；1 小时后再次采样，服务端已把重置大幅推远
        let before = snap_at(base, Some(base + 20 * 3600_000));
        let after_ts = base + 3600_000;
        let after = snap_at(after_ts, Some(after_ts + 8 * 86_400_000));
        let periods = split_periods(&[before, after]);
        assert_eq!(periods.len(), 2);
        // 新周期起点 = min(预告时刻, 首条快照 ts) = 快照 ts（无缝，不取未到的预告时刻）
        assert_eq!(periods[1].reset_at, after_ts);
        // 上一周期 end_at 与新周期 reset_at 共用同一分界：提前重置场景下
        // 上一周期 end_at 也必须收窄到快照 ts，否则重叠段用量会被双计
        assert_eq!(periods[0].end_at, after_ts);
        assert!(periods[0].end_at <= periods[1].reset_at, "相邻周期必须无缝不重叠");
        assert!(periods[1].is_current);
    }

    /// 周期切分：无跳变时只有单个当前周期，起点用首条快照 ts 兜底
    #[test]
    fn split_periods_single_when_no_jump() {
        let base = 1_800_000_000_000_i64;
        let snaps = vec![
            snap_at(base, Some(base + 7 * 86_400_000)),
            snap_at(base + 3600_000, Some(base + 7 * 86_400_000)),
        ];
        let periods = split_periods(&snaps);
        assert_eq!(periods.len(), 1, "weekly_reset 未跳变不应切分周期");
        assert_eq!(periods[0].reset_at, base);
        assert_eq!(periods[0].sample_count, 2);
        assert!(periods[0].is_current);
    }

    /// 尾部回读：常规多行文件能取到最后一条的 ts
    #[test]
    fn read_last_ts_returns_last_line() {
        let path = temp_jsonl("last");
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&snap(1000, "pro")).unwrap(),
            serde_json::to_string(&snap(2000, "pro")).unwrap(),
            serde_json::to_string(&snap(3000, "max")).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        assert_eq!(read_last_ts(&path).unwrap(), 3000, "应返回最后一条快照的 ts");
        std::fs::remove_file(&path).ok();
    }

    /// 尾部回读：单行 JSON 超过初始 2KB 窗口时逐倍扩大窗口仍能取到
    #[test]
    fn read_last_ts_expands_window_for_long_line() {
        let path = temp_jsonl("long");
        let long = snap(42, &"x".repeat(5000));
        let content = format!(
            "{}\n{}\n",
            serde_json::to_string(&snap(1000, "pro")).unwrap(),
            serde_json::to_string(&long).unwrap()
        );
        std::fs::write(&path, &content).unwrap();
        assert_eq!(read_last_ts(&path).unwrap(), 42, "长行应通过扩大窗口解析到");
        std::fs::remove_file(&path).ok();
    }

    /// 尾部回读：空文件返回 0（视作无历史，防抖直接放行）
    #[test]
    fn read_last_ts_empty_file_returns_zero() {
        let path = temp_jsonl("empty");
        std::fs::write(&path, "").unwrap();
        assert_eq!(read_last_ts(&path).unwrap(), 0, "空文件应返回 0");
        std::fs::remove_file(&path).ok();
    }

    /// 滚动清理：过期行被删、保留行原样保留，重写后仍可正常解析
    #[test]
    fn cleanup_removes_expired_lines() {
        let path = temp_jsonl("cleanup");
        let now = chrono::Local::now().timestamp_millis();
        let old_ts = now - RETENTION_MS - 86_400_000; // 超过保留期一天
        let content = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&snap(old_ts, "old")).unwrap(),
            serde_json::to_string(&snap(now - 1000, "keep1")).unwrap(),
            serde_json::to_string(&snap(now, "keep2")).unwrap()
        );
        std::fs::write(&path, &content).unwrap();

        try_cleanup(&path).unwrap();

        let cleaned = std::fs::read_to_string(&path).unwrap();
        assert!(!cleaned.contains("old"), "过期行应被删除");
        assert_eq!(cleaned.lines().count(), 2, "保留行应原样保留");
        assert_eq!(read_last_ts(&path).unwrap(), now);
        std::fs::remove_file(&path).ok();
    }

    /// 残行补丁：文件尾是无换行的半截残行时补 \n，正常文件不动
    #[test]
    fn ensure_trailing_newline_patches_partial_line() {
        let path = temp_jsonl("newline");
        let ok_line = serde_json::to_string(&snap(1000, "pro")).unwrap();
        // 模拟进程崩溃残留：尾行是半截 JSON 且无换行
        std::fs::write(&path, format!("{ok_line}\n{{\"ts\":2000,\"level\":\"pr"))
            .unwrap();
        ensure_trailing_newline(&path).unwrap();
        let patched = std::fs::read_to_string(&path).unwrap();
        assert!(patched.ends_with('\n'), "残行后应补上换行");
        assert_eq!(patched.lines().count(), 2, "不应改变行数");

        // 已正常结尾的文件：幂等，不再追加换行
        ensure_trailing_newline(&path).unwrap();
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            patched,
            "正常结尾时不应追加换行"
        );
        std::fs::remove_file(&path).ok();
    }
}

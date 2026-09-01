//! Codex / Claude / Cursor 额度快照历史。
//!
//! 与智谱的 quota_history 分开存储：智谱历史有专用的周额度对比协议，
//! Agent 额度窗口则使用可扩展的 windows 数组，供今日增量和多设备同步使用。

use chrono::Local;
#[cfg(test)]
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, SystemTime};

use crate::pricing::config_dir;

const RETENTION_MS: i64 = 90 * 86_400_000;
const CLEANUP_INTERVAL: Duration = Duration::from_secs(24 * 3600);

static LAST_CLEANUP: OnceLock<Mutex<Option<SystemTime>>> = OnceLock::new();
static APPEND_LOCK: OnceLock<Mutex<()>> = OnceLock::new();
static LAST_TS_BY_SOURCE: OnceLock<Mutex<HashMap<String, i64>>> = OnceLock::new();
/// 本进程已按计费周期清理过失真 Cursor 样本的标记：(reset_at, 当时的周期已用百分比)。
static INFLATED_CURSOR_CLEANED: OnceLock<Mutex<Option<(Option<i64>, f64)>>> = OnceLock::new();

/// 一个 Agent 额度窗口的快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentQuotaWindow {
    /// 稳定窗口键：hour5 / weekly / cursor_auto / cursor_api。
    pub key: String,
    /// 已用百分比（0-100）。
    pub used_pct: f64,
    /// 窗口下次重置时间（毫秒）；Cursor 使用计费周期结束时间。
    pub reset_at: Option<i64>,
}

/// 一个 Agent 在某个时刻的完整额度快照。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct AgentQuotaSnapshot {
    /// 数据来源：codex / claude / cursor。
    pub source: String,
    /// 采样时间（毫秒）。
    pub ts: i64,
    /// 套餐类型，例如 plus / pro / max。
    pub plan_type: Option<String>,
    /// 当前可用的额度窗口。
    pub windows: Vec<AgentQuotaWindow>,
}

pub fn history_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("agent_quota_history.jsonl"))
}

/// 追加一条快照。同一来源同一秒内的重复调用只保留第一条，
/// 避免同一轮不同时间范围查询把同一份实时额度重复计入采样数。
pub fn append_snapshot(snapshot: &AgentQuotaSnapshot) {
    if snapshot.source.trim().is_empty() {
        return;
    }
    let mut sanitized = snapshot.clone();
    sanitized.windows.retain(|window| is_valid_used_pct(window.used_pct));
    if sanitized.windows.is_empty() {
        return;
    }
    if let Err(e) = try_append(&sanitized) {
        eprintln!("[zbar-agent-history] 写额度快照失败: {e}");
    }
}

fn try_append(snapshot: &AgentQuotaSnapshot) -> Result<(), String> {
    let append_lock = APPEND_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = append_lock
        .lock()
        .map_err(|_| "Agent 额度快照写入锁已中毒".to_string())?;
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = history_path()?;

    maybe_cleanup(&path);
    if path.exists() {
        ensure_trailing_newline(&path)?;
    }

    let seen = LAST_TS_BY_SOURCE.get_or_init(|| {
        let mut values: HashMap<String, i64> = HashMap::new();
        if let Ok(snapshots) = load_all() {
            for old in snapshots {
                values
                    .entry(old.source)
                    .and_modify(|ts| *ts = (*ts).max(old.ts))
                    .or_insert(old.ts);
            }
        }
        Mutex::new(values)
    });
    let mut seen = seen
        .lock()
        .map_err(|_| "Agent 额度快照去重锁已中毒".to_string())?;
    if let Some(last_ts) = seen.get(&snapshot.source) {
        if same_second(*last_ts, snapshot.ts) {
            return Ok(());
        }
    }

    let line =
        serde_json::to_string(snapshot).map_err(|e| format!("序列化 Agent 额度快照失败: {e}"))?;
    let mut file = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|e| format!("打开 Agent 额度快照文件失败: {e}"))?;
    writeln!(file, "{line}").map_err(|e| format!("写入 Agent 额度快照失败: {e}"))?;
    seen.entry(snapshot.source.clone())
        .and_modify(|last_ts| *last_ts = (*last_ts).max(snapshot.ts))
        .or_insert(snapshot.ts);
    Ok(())
}

fn same_second(left_ms: i64, right_ms: i64) -> bool {
    left_ms / 1000 == right_ms / 1000
}

fn is_valid_used_pct(value: f64) -> bool {
    value.is_finite() && (0.0..=100.0).contains(&value)
}

/// 进程异常退出后，若文件尾没有换行，先补齐边界再追加新快照。
fn ensure_trailing_newline(path: &std::path::Path) -> Result<(), String> {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .map_err(|e| format!("打开 Agent 额度快照文件失败: {e}"))?;
    let len = file
        .seek(SeekFrom::End(0))
        .map_err(|e| format!("定位 Agent 额度快照文件失败: {e}"))?;
    if len == 0 {
        return Ok(());
    }
    let mut last = [0u8; 1];
    file.seek(SeekFrom::Start(len - 1))
        .map_err(|e| format!("定位 Agent 额度快照文件失败: {e}"))?;
    file.read_exact(&mut last)
        .map_err(|e| format!("读取 Agent 额度快照末字节失败: {e}"))?;
    if last[0] != b'\n' {
        file.seek(SeekFrom::End(0))
            .map_err(|e| format!("定位 Agent 额度快照文件失败: {e}"))?;
        file.write_all(b"\n")
            .map_err(|e| format!("补写 Agent 额度快照换行失败: {e}"))?;
    }
    Ok(())
}

fn maybe_cleanup(path: &PathBuf) {
    let cell = LAST_CLEANUP.get_or_init(|| Mutex::new(None));
    let Ok(mut last) = cell.lock() else {
        return;
    };
    if let Some(t) = *last {
        if t.elapsed().unwrap_or(Duration::ZERO) < CLEANUP_INTERVAL {
            return;
        }
    }
    *last = Some(SystemTime::now());
    drop(last);

    if let Err(e) = try_cleanup(path) {
        eprintln!("[zbar-agent-history] 滚动清理失败（24h 后重试）: {e}");
    }
}

fn try_cleanup(path: &std::path::Path) -> Result<(), String> {
    if !path.exists() {
        return Ok(());
    }
    let cutoff = Local::now().timestamp_millis() - RETENTION_MS;
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开 Agent 额度快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            _ => continue,
        };
        match serde_json::from_str::<AgentQuotaSnapshot>(&line) {
            Ok(snapshot) if snapshot.ts < cutoff => removed += 1,
            _ => kept.push(line),
        }
    }
    if removed == 0 {
        return Ok(());
    }

    let tmp = path.with_extension("jsonl.tmp");
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("打开 Agent 额度临时文件失败: {e}"))?;
    for line in kept {
        writeln!(out, "{line}").map_err(|e| format!("写入 Agent 额度临时文件失败: {e}"))?;
    }
    out.flush()
        .map_err(|e| format!("刷新 Agent 额度临时文件失败: {e}"))?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| format!("替换 Agent 额度快照文件失败: {e}"))
}

/// 清理历史上被混用口径污染的 Cursor 快照。
///
/// 修复前的 Cursor 今日增量逻辑曾把「周期已用美元」与「周期已用百分比」两套口径
/// 混用，合成出远超真实周期的百分比快照。此类失真值满足逻辑必然：同一计费周期内
/// （同 reset_at），真实百分比不可能超过当前周期已用百分比，超出的必然是失真样本。
/// 同一 reset_at 只清理一次，避免每次拉取都全量扫描重写。
pub fn remove_inflated_cursor_samples(reset_at: Option<i64>, current_used_pct: f64) {
    let cell = INFLATED_CURSOR_CLEANED.get_or_init(|| Mutex::new(None));
    let Ok(marker) = cell.lock() else {
        return;
    };
    if let Some((cleaned_reset_at, _)) = *marker {
        if cleaned_reset_at == reset_at {
            return;
        }
    }
    drop(marker);

    let path = match history_path() {
        Ok(path) => path,
        Err(e) => {
            eprintln!("[zbar-agent-history] 清理 Cursor 失真快照失败: {e}");
            return;
        }
    };
    match try_remove_inflated_cursor_samples(&path, reset_at, current_used_pct) {
        Ok(()) => {
            if let Some(cell) = INFLATED_CURSOR_CLEANED.get() {
                if let Ok(mut marker) = cell.lock() {
                    *marker = Some((reset_at, current_used_pct));
                }
            }
        }
        Err(e) => eprintln!("[zbar-agent-history] 清理 Cursor 失真快照失败: {e}"),
    }
}

fn try_remove_inflated_cursor_samples(
    path: &std::path::Path,
    reset_at: Option<i64>,
    current_used_pct: f64,
) -> Result<(), String> {
    let append_lock = APPEND_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = append_lock
        .lock()
        .map_err(|_| "Agent 额度快照写入锁已中毒".to_string())?;
    if !path.exists() {
        return Ok(());
    }
    let file = OpenOptions::new()
        .read(true)
        .open(path)
        .map_err(|e| format!("打开 Agent 额度快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut kept = Vec::new();
    let mut removed = 0usize;
    for line in reader.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            _ => continue,
        };
        let is_inflated = serde_json::from_str::<AgentQuotaSnapshot>(&line)
            .map(|snapshot| {
                snapshot.source == "cursor"
                    && snapshot.windows.iter().any(|window| {
                        window.key == "cursor_auto"
                            && window.reset_at == reset_at
                            && window.used_pct > current_used_pct + 0.5
                    })
            })
            .unwrap_or(false);
        if is_inflated {
            removed += 1;
        } else {
            kept.push(line);
        }
    }
    if removed == 0 {
        return Ok(());
    }

    let tmp = path.with_extension("jsonl.tmp");
    let mut out = OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("打开 Agent 额度临时文件失败: {e}"))?;
    for line in kept {
        writeln!(out, "{line}").map_err(|e| format!("写入 Agent 额度临时文件失败: {e}"))?;
    }
    out.flush()
        .map_err(|e| format!("刷新 Agent 额度临时文件失败: {e}"))?;
    drop(out);
    fs::rename(&tmp, path).map_err(|e| format!("替换 Agent 额度快照文件失败: {e}"))?;

    // cursor 来源的 last ts 缓存可能指向已删除行，移除后下次 append 会正常写入
    if let Some(seen) = LAST_TS_BY_SOURCE.get() {
        if let Ok(mut seen) = seen.lock() {
            seen.remove("cursor");
        }
    }
    Ok(())
}

/// 读取全部快照，损坏行跳过并按时间升序返回。
pub fn load_all() -> Result<Vec<AgentQuotaSnapshot>, String> {
    let path = history_path()?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let file = OpenOptions::new()
        .read(true)
        .open(&path)
        .map_err(|e| format!("打开 Agent 额度快照文件失败: {e}"))?;
    let reader = BufReader::new(file);
    let mut out = Vec::new();
    for line in reader.lines() {
        let line = match line {
            Ok(line) if !line.trim().is_empty() => line,
            _ => continue,
        };
        if let Ok(snapshot) = serde_json::from_str::<AgentQuotaSnapshot>(&line) {
            out.push(snapshot);
        }
    }
    out.sort_by_key(|snapshot| snapshot.ts);
    Ok(out)
}

/// 读取指定时间范围内的本地快照。
pub fn load_range(from_ms: i64, to_ms: i64) -> Result<Vec<AgentQuotaSnapshot>, String> {
    Ok(load_all()?
        .into_iter()
        .filter(|snapshot| snapshot.ts >= from_ms && snapshot.ts < to_ms)
        .collect())
}

/// 删除 Agent 额度快照历史。
pub fn clear_history() -> Result<(), String> {
    let append_lock = APPEND_LOCK.get_or_init(|| Mutex::new(()));
    let _guard = append_lock
        .lock()
        .map_err(|_| "Agent 额度快照写入锁已中毒".to_string())?;
    let path = history_path()?;
    if path.exists() {
        fs::remove_file(path).map_err(|e| format!("删除 Agent 额度快照文件失败: {e}"))?;
    }
    if let Some(seen) = LAST_TS_BY_SOURCE.get() {
        seen.lock()
            .map_err(|_| "Agent 额度快照去重锁已中毒".to_string())?
            .clear();
    }
    Ok(())
}

/// 本地时区今日零点（毫秒）。
#[cfg(test)]
pub fn today_start_ms() -> i64 {
    Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| Local.from_local_datetime(&naive).single())
        .map(|value| value.timestamp_millis())
        .unwrap_or_else(|| Local::now().timestamp_millis())
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Timelike;

    fn sample(ts: i64, pct: f64, reset_at: Option<i64>) -> AgentQuotaSnapshot {
        AgentQuotaSnapshot {
            source: "codex".into(),
            ts,
            plan_type: Some("pro".into()),
            windows: vec![AgentQuotaWindow {
                key: "weekly".into(),
                used_pct: pct,
                reset_at,
            }],
        }
    }

    #[test]
    fn snapshot_round_trip() {
        let value = sample(1_700_000_000_000, 12.5, Some(1_700_604_800_000));
        let json = serde_json::to_string(&value).unwrap();
        let parsed: AgentQuotaSnapshot = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, value);
    }

    #[test]
    fn today_start_is_at_midnight() {
        let start = today_start_ms();
        let local = Local.timestamp_millis_opt(start).single().unwrap();
        assert_eq!(local.hour(), 0);
        assert_eq!(local.minute(), 0);
        assert_eq!(local.second(), 0);
    }

    #[test]
    fn invalid_percentages_are_not_persisted() {
        let mut value = sample(1_700_000_000_000, -1.0, None);
        value.windows.push(AgentQuotaWindow {
            key: "hour5".into(),
            used_pct: 101.0,
            reset_at: None,
        });
        value.windows.push(AgentQuotaWindow {
            key: "weekly".into(),
            used_pct: 20.0,
            reset_at: None,
        });
        value
            .windows
            .retain(|window| is_valid_used_pct(window.used_pct));
        assert_eq!(value.windows.len(), 1);
        assert_eq!(value.windows[0].used_pct, 20.0);
    }

    #[test]
    fn same_second_is_deduplicated() {
        assert!(same_second(1_700_000_000_123, 1_700_000_000_999));
        assert!(!same_second(1_700_000_000_999, 1_700_000_001_000));
    }

    #[test]
    fn cleanup_removes_only_expired_snapshots() {
        let path = std::env::temp_dir().join(format!(
            "zbar-agent-history-test-{}-{}.jsonl",
            std::process::id(),
            // 线程名含 "::"（模块路径），Windows 文件名不允许冒号
            std::thread::current()
                .name()
                .unwrap_or("cleanup")
                .replace(':', "_")
        ));
        let expired = sample(
            Local::now().timestamp_millis() - RETENTION_MS - 1,
            10.0,
            None,
        );
        let current = sample(Local::now().timestamp_millis(), 20.0, None);
        let contents = format!(
            "{}\n{}\n",
            serde_json::to_string(&expired).unwrap(),
            serde_json::to_string(&current).unwrap()
        );
        std::fs::write(&path, contents).unwrap();

        try_cleanup(&path).unwrap();
        let lines = std::fs::read_to_string(&path).unwrap();
        let snapshots: Vec<AgentQuotaSnapshot> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(snapshots, vec![current]);
        let _ = std::fs::remove_file(path);
    }

    fn cursor_sample(ts: i64, key: &str, pct: f64, reset_at: Option<i64>) -> AgentQuotaSnapshot {
        AgentQuotaSnapshot {
            source: "cursor".into(),
            ts,
            plan_type: Some("pro".into()),
            windows: vec![AgentQuotaWindow {
                key: key.into(),
                used_pct: pct,
                reset_at,
            }],
        }
    }

    #[test]
    fn inflated_cursor_cleanup_removes_only_inflated_samples() {
        let path = std::env::temp_dir().join(format!(
            "zbar-agent-history-test-{}-{}.jsonl",
            std::process::id(),
            // 线程名含 "::"（模块路径），Windows 文件名不允许冒号
            std::thread::current()
                .name()
                .unwrap_or("inflated")
                .replace(':', "_")
        ));
        let reset_at = Some(1_700_604_800_000i64);
        // 同周期正常值：未超过当前已用百分比，保留
        let normal = cursor_sample(1_700_000_000_000, "cursor_auto", 19.78, reset_at);
        // 同周期失真合成值：超过当前已用百分比 + 容差，删除
        let inflated = cursor_sample(1_700_000_100_000, "cursor_auto", 65.02, reset_at);
        // 不同周期的高百分比：不属于本周期，保留
        let other_cycle = cursor_sample(
            1_700_000_200_000,
            "cursor_auto",
            65.02,
            Some(1_700_864_000_000),
        );
        let contents = format!(
            "{}\n{}\n{}\n",
            serde_json::to_string(&normal).unwrap(),
            serde_json::to_string(&inflated).unwrap(),
            serde_json::to_string(&other_cycle).unwrap()
        );
        std::fs::write(&path, contents).unwrap();

        try_remove_inflated_cursor_samples(&path, reset_at, 27.18).unwrap();
        let lines = std::fs::read_to_string(&path).unwrap();
        let snapshots: Vec<AgentQuotaSnapshot> = lines
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .collect();
        assert_eq!(snapshots, vec![normal, other_cycle]);
        let _ = std::fs::remove_file(path);
    }
}

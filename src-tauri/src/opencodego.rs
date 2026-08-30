//! OpenCode Go（本地 SQLite 用量估算）模块。
//!
//! 无凭证、无网络：直读 OpenCode 本地数据目录的 opencode.db（message 单表，
//! 不联合 part 表），按 CodexBar 同款常量估算滚动 5 小时窗 / 周 / 月三档
//! 用量百分比。数据目录：`~/.local/share/opencode/`（Windows 上 OpenCode
//! 同样使用 `%USERPROFILE%\.local\share\opencode\`）；`ZBAR_OPENCODE_HOME`
//! 可覆盖根目录（测试/便携场景用，对齐 codex.rs 的 ZBAR_CODEX_HOME 先例）。
//!
//! 登录检测：auth.json 顶层键 `"opencode-go"` 内层 `"key"` 非空——key 本身
//! 不参与查询，只用于判断是否启用了 Go 计划。库不存在 → 返回空 Vec
//! （前端 tab 不出现，与 has_local_data 的 presence 口径一致）；库存在但
//! 未登录 Go 计划 → 返回一条 pending 条目提示。
//!
//! 估算口径（CodexBar 常量）：滚动 5h 窗限额 $12、周（UTC 周一起）$30、
//! 月 $60；percent = used/limit*100（clamp 0-100）。5h 窗重置 = 窗口内最老
//! 一条 + 5h；周/月重置 = 下个周一 / 下月 1 日 00:00 UTC。这是本地用量
//! 推算，非官方额度数据（entry.message 注明）。
//!
//! 工程纪律（对齐 kimi.rs / codex.rs 先例）：
//! - 只读打开：SQLITE_OPEN_READ_ONLY + busy_timeout 250ms，绝不写用户库；
//! - 网络：无（纯本地文件 IO），调用方仍按惯例 spawn_blocking 卸载。

use crate::provider_quota::{now_ms, ProviderQuotaEntry, ProviderQuotaWindow};
use rusqlite::{Connection, OpenFlags};
use std::path::{Path, PathBuf};

/// CodexBar 同款估算常量：5 小时滚动窗 / 周 / 月限额（美元）。
const LIMIT_5H_USD: f64 = 12.0;
const LIMIT_WEEK_USD: f64 = 30.0;
const LIMIT_MONTH_USD: f64 = 60.0;
/// 5 小时滚动窗长度（毫秒）。
const FIVE_HOURS_MS: i64 = 5 * 3_600_000;
/// 周窗长度（毫秒）。
const WEEK_MS: i64 = 7 * 86_400_000;

/// OpenCode 数据根目录（ZBAR_OPENCODE_HOME 优先，其次 ~/.local/share/opencode/）。
fn opencode_root() -> PathBuf {
    if let Ok(home) = std::env::var("ZBAR_OPENCODE_HOME") {
        let home = home.trim();
        if !home.is_empty() {
            return PathBuf::from(home);
        }
    }
    dirs::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".local")
        .join("share")
        .join("opencode")
}

/// 本地库路径（presence 探测与查询共用同一口径）。
fn db_path() -> PathBuf {
    opencode_root().join("opencode.db")
}

/// 本地数据是否可用（provider_credentials::has_credentials 对 opencodego 的
/// presence 特判用）：opencode.db 存在即视为可用，装了 OpenCode 的用户
/// tab 自动出现；凭证体系对其保持可选。
pub(crate) fn has_local_data() -> bool {
    db_path().exists()
}

/// 登录检测：auth.json 顶层 "opencode-go" 内层 "key" 非空（文件缺失/
/// 解析失败/键缺失一律视为未登录，不阻断主流程）。
fn has_go_login(root: &Path) -> bool {
    let path = root.join("auth.json");
    let Ok(data) = std::fs::read_to_string(&path) else {
        return false;
    };
    let Ok(v) = serde_json::from_str::<serde_json::Value>(&data) else {
        return false;
    };
    v.get("opencode-go")
        .and_then(|go| go.get("key"))
        .and_then(|k| k.as_str())
        .map(|k| !k.trim().is_empty())
        .unwrap_or(false)
}

/// 以只读方式打开本地库：READONLY（绝不写用户数据）+ busy_timeout 250ms
/// （OpenCode 正在写入时短暂等待而非立即报错；估算用途失败可容忍）。
fn open_readonly(path: &Path) -> Result<Connection, String> {
    let conn = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("打开 OpenCode 本地库失败: {e}"))?;
    conn.busy_timeout(std::time::Duration::from_millis(250))
        .map_err(|e| format!("设置 busy_timeout 失败: {e}"))?;
    Ok(conn)
}

/// 读取 opencode-go 的 (created_ms, cost_usd) 序列。
/// 只查 message 单表：providerID=opencode-go 的 assistant 消息，cost 为
/// 数值（json_type 过滤字符串脏值）；时间取 COALESCE($.time.created,
/// time_created)（毫秒 epoch），任一时间都拿不到的行无法定位窗口，跳过。
fn query_records(conn: &Connection) -> Result<Vec<(i64, f64)>, String> {
    let mut stmt = conn
        .prepare(
            "SELECT COALESCE(json_extract(data,'$.time.created'), time_created), \
                    json_extract(data,'$.cost') \
             FROM message \
             WHERE json_extract(data,'$.providerID')='opencode-go' \
               AND json_extract(data,'$.role')='assistant' \
               AND json_type(data,'$.cost') IN ('integer','real')",
        )
        .map_err(|e| format!("OpenCode 查询准备失败: {e}"))?;
    let rows = stmt
        .query_map([], |row| {
            let ts: rusqlite::types::Value = row.get(0)?;
            let cost: f64 = row.get(1)?;
            Ok((ts, cost))
        })
        .map_err(|e| format!("OpenCode 用量查询失败: {e}"))?;
    let mut records = Vec::new();
    for row in rows {
        let (ts, cost) = row.map_err(|e| format!("OpenCode 用量读取失败: {e}"))?;
        let ts_ms = match ts {
            rusqlite::types::Value::Integer(ms) => ms,
            rusqlite::types::Value::Real(ms) => ms as i64,
            _ => continue, // COALESCE 后仍为 NULL：无法定位窗口，跳过
        };
        // 负值成本属脏数据，忽略
        if cost.is_finite() && cost >= 0.0 {
            records.push((ts_ms, cost));
        }
    }
    Ok(records)
}

/// 日历窗口边界（纯函数，now 注入便于单测；全部按 UTC 口径）：
/// 5h 滚动窗起点、本周（周一 00:00）起止、本月（1 日 00:00）起止。
struct WindowBounds {
    h5_start: i64,
    week_start: i64,
    week_reset: i64,
    month_start: i64,
    month_reset: i64,
}

fn window_bounds(now: i64) -> WindowBounds {
    use chrono::{Datelike, TimeZone};
    let dt = chrono::Utc
        .timestamp_millis_opt(now)
        .single()
        .unwrap_or_else(chrono::Utc::now);
    // 当日 00:00 UTC（毫秒）
    let midnight = dt
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(now);
    // 周一 = 00:00 - 距周一的天数
    let week_start = midnight - (dt.weekday().num_days_from_monday() as i64) * 86_400_000;
    // 月窗简化为自然月（UTC）：本月 1 日 00:00 起，下月 1 日 00:00 重置
    let (year, month) = (dt.year(), dt.month());
    let month_start = chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(0);
    let (next_year, next_month) = if month == 12 {
        (year + 1, 1)
    } else {
        (year, month + 1)
    };
    let month_reset = chrono::NaiveDate::from_ymd_opt(next_year, next_month, 1)
        .and_then(|d| d.and_hms_opt(0, 0, 0))
        .map(|n| chrono::Utc.from_utc_datetime(&n).timestamp_millis())
        .unwrap_or(month_start + 30 * 86_400_000);
    WindowBounds {
        h5_start: now - FIVE_HOURS_MS,
        week_start,
        week_reset: week_start + WEEK_MS,
        month_start,
        month_reset,
    }
}

/// 单窗百分比：used/limit*100，clamp 0-100（超限按 100 展示，不炸进度条）。
fn percent_of(used: f64, limit: f64) -> f64 {
    (used / limit * 100.0).clamp(0.0, 100.0)
}

/// 窗口聚合（纯函数，now 注入便于单测）：滚动 5h 主窗 + 周副窗 + 月第三窗。
/// 5h 窗重置 = 窗口内最老一条 + 5h（窗内无记录则不展示重置时间）；
/// 周/月窗按日历边界累计。
fn build_entry(now: i64, records: &[(i64, f64)]) -> ProviderQuotaEntry {
    let bounds = window_bounds(now);
    let mut used5h = 0.0;
    let mut used_week = 0.0;
    let mut used_month = 0.0;
    // 5h 窗内最老一条（重置时间锚点）
    let mut oldest_in_5h: Option<i64> = None;
    for (ts, cost) in records {
        if *ts >= bounds.h5_start {
            used5h += cost;
            oldest_in_5h = Some(oldest_in_5h.map_or(*ts, |old| old.min(*ts)));
        }
        if *ts >= bounds.week_start {
            used_week += cost;
        }
        if *ts >= bounds.month_start {
            used_month += cost;
        }
    }
    let reset5h = oldest_in_5h.map(|old| old + FIVE_HOURS_MS);

    let mk_window = |key: &str, title: &str, used: f64, limit: f64, resets_at: Option<i64>| {
        ProviderQuotaWindow {
            key: key.to_string(),
            title: title.to_string(),
            used_percent: Some(percent_of(used, limit)),
            used: Some(used),
            total: Some(limit),
            unit: Some("$".to_string()),
            resets_at,
        }
    };

    ProviderQuotaEntry {
        credential_id: "local".to_string(),
        label: "本地估算".to_string(),
        status: "ok".to_string(),
        windows: vec![
            mk_window("hour5", "5 小时窗口", used5h, LIMIT_5H_USD, reset5h),
            mk_window("weekly", "本周", used_week, LIMIT_WEEK_USD, Some(bounds.week_reset)),
            mk_window("monthly", "本月", used_month, LIMIT_MONTH_USD, Some(bounds.month_reset)),
        ],
        balance: None,
        plan_name: None,
        // 本地推算非官方口径，message 注明避免误导
        message: Some("本地用量估算，非官方额度数据".to_string()),
        updated_at: now_ms(),
    }
}

/// 读取 OpenCode Go 用量并产出单条展示条目（无网络，纯本地文件 IO）：
/// - 库不存在 → 空 Vec（前端 tab 不出现，presence 同口径）；
/// - 库存在但 auth.json 无 opencode-go 登录 → 单条 pending 提示；
/// - 打开/查询失败 → 单条 error（中文原因）；
/// - 成功 → ok 条目（5h/周/月三窗估算）。
pub(crate) fn fetch_quota_entries() -> Vec<ProviderQuotaEntry> {
    let root = opencode_root();
    let db = db_path();
    if !db.exists() {
        return vec![];
    }
    if !has_go_login(&root) {
        return vec![ProviderQuotaEntry {
            credential_id: "local".to_string(),
            label: "本地估算".to_string(),
            status: "pending".to_string(),
            windows: vec![],
            balance: None,
            plan_name: None,
            message: Some("未检测到 OpenCode Go 订阅登录".to_string()),
            updated_at: now_ms(),
        }];
    }
    let records = match open_readonly(&db).and_then(|conn| query_records(&conn)) {
        Ok(records) => records,
        Err(e) => {
            return vec![ProviderQuotaEntry {
                credential_id: "local".to_string(),
                label: "本地估算".to_string(),
                status: "error".to_string(),
                windows: vec![],
                balance: None,
                plan_name: None,
                message: Some(e),
                updated_at: now_ms(),
            }];
        }
    };
    vec![build_entry(now_ms(), &records)]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 已知锚点：2024-01-01（周一）00:00:00 UTC = 1704067200000 ms。
    const MON_0: i64 = 1_704_067_200_000;
    /// 2024-01-03（周三）12:00:00 UTC = 1704273600000 ms。
    const WED_NOON: i64 = MON_0 + 2 * 86_400_000 + 12 * 3_600_000;
    /// 2024-02-01 00:00:00 UTC = 1706745600000 ms。
    const FEB_1: i64 = 1_706_745_600_000;

    /// 建内存库并插入 message 样例行（data JSON + time_created）。
    fn memory_db() -> Connection {
        let conn = Connection::open_in_memory().expect("打开内存库失败");
        conn.execute_batch(
            "CREATE TABLE message (
                id INTEGER PRIMARY KEY,
                data TEXT NOT NULL,
                time_created INTEGER
             );",
        )
        .expect("建表失败");
        conn
    }

    fn insert_row(conn: &Connection, data: &str, time_created: Option<i64>) {
        conn.execute(
            "INSERT INTO message (data, time_created) VALUES (?1, ?2)",
            rusqlite::params![data, time_created],
        )
        .expect("插入失败");
    }

    /// 窗口聚合：5h 滚动窗 / 周 / 月各自累计 + 重置时间锚定。
    #[test]
    fn aggregates_windows_with_reset_anchors() {
        let in_5h_a = WED_NOON - 3_600_000; // 窗内（1h 前）
        let in_5h_b = WED_NOON - 3 * 3_600_000; // 窗内（3h 前，最老）
        let out_5h = WED_NOON - 10 * 3_600_000; // 出 5h 窗，在本周本月
        let week_only = MON_0 + 3_600_000; // 周一凌晨：本周本月
        let records = vec![
            (in_5h_a, 2.0),
            (in_5h_b, 1.5),
            (out_5h, 4.0),
            (week_only, 0.5),
        ];
        let entry = build_entry(WED_NOON, &records);
        assert_eq!(entry.status, "ok");
        assert_eq!(entry.credential_id, "local");
        assert_eq!(entry.label, "本地估算");
        assert_eq!(entry.windows.len(), 3);

        // 5h 主窗：2.0 + 1.5 = 3.5 → 3.5/12 = 29.17%；重置 = 最老一条 + 5h
        let h5 = &entry.windows[0];
        assert_eq!(h5.key, "hour5");
        assert_eq!(h5.title, "5 小时窗口");
        assert!((h5.used_percent.unwrap() - 3.5 / 12.0 * 100.0).abs() < 1e-9);
        assert_eq!(h5.used, Some(3.5));
        assert_eq!(h5.total, Some(12.0));
        assert_eq!(h5.resets_at, Some(in_5h_b + FIVE_HOURS_MS));

        // 周窗：全部 4 条都在本周 → 8.0/30；重置 = 下周一 00:00 UTC
        let weekly = &entry.windows[1];
        assert_eq!(weekly.key, "weekly");
        assert_eq!(weekly.used, Some(8.0));
        assert!((weekly.used_percent.unwrap() - 8.0 / 30.0 * 100.0).abs() < 1e-9);
        assert_eq!(weekly.resets_at, Some(MON_0 + WEEK_MS));

        // 月窗：同样全部在本月 → 8.0/60；重置 = 下月 1 日 00:00 UTC
        let monthly = &entry.windows[2];
        assert_eq!(monthly.key, "monthly");
        assert_eq!(monthly.used, Some(8.0));
        assert!((monthly.used_percent.unwrap() - 8.0 / 60.0 * 100.0).abs() < 1e-9);
        assert_eq!(monthly.resets_at, Some(FEB_1));

        // 估算口径声明
        assert_eq!(
            entry.message.as_deref(),
            Some("本地用量估算，非官方额度数据")
        );
    }

    /// 百分比 clamp：单条超大花费把 5h 窗打满也只展示 100%，不为负/超百。
    #[test]
    fn percent_clamps_to_0_100() {
        let entry = build_entry(WED_NOON, &[(WED_NOON - 1000, 999.0)]);
        let h5 = &entry.windows[0];
        assert_eq!(h5.used_percent, Some(100.0));
        // 空 records：三窗 used=0 → percent=0，5h 无重置时间
        let entry = build_entry(WED_NOON, &[]);
        for w in &entry.windows {
            assert_eq!(w.used_percent, Some(0.0));
            assert_eq!(w.used, Some(0.0));
        }
        assert_eq!(entry.windows[0].resets_at, None);
        // 周/月窗仍有日历重置时间
        assert_eq!(entry.windows[1].resets_at, Some(MON_0 + WEEK_MS));
    }

    /// 查询过滤：providerID/role 不符排除、cost 字符串脏值排除、
    /// 时间 COALESCE（data 缺 time.created 时回退 time_created 列）、
    /// 两处时间全缺的行跳过。
    #[test]
    fn query_filters_and_coalesce() {
        let conn = memory_db();
        // 合法行 A：data 带 time.created（毫秒），cost 数值
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant",
                "time":{"created":1704270000000},"cost":1.25}"#,
            Some(111111),
        );
        // 合法行 B：data 缺 time.created → COALESCE 回退 time_created 列
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":2}"#,
            Some(WED_NOON - 3_600_000),
        );
        // 排除：其他 provider
        insert_row(
            &conn,
            r#"{"providerID":"anthropic","role":"assistant","cost":99.0,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：非 assistant
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"user","cost":99.0,
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 排除：cost 为字符串脏值（json_type 不在 integer/real）
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":"3.0",
                "time":{"created":1704270000000}}"#,
            None,
        );
        // 跳过：cost 合法但 data 与列的时间全缺
        insert_row(
            &conn,
            r#"{"providerID":"opencode-go","role":"assistant","cost":5.0}"#,
            None,
        );

        let records = query_records(&conn).expect("查询应成功");
        // 行 A 取 data 时间；行 B 回退 time_created；其余被过滤
        assert_eq!(
            records,
            vec![(1_704_270_000_000, 1.25), (WED_NOON - 3_600_000, 2.0)]
        );

        // 聚合口径：两条都在 5h 窗内（WED_NOON 视角）
        let entry = build_entry(WED_NOON, &records);
        assert_eq!(entry.windows[0].used, Some(3.25));
        // 行 B（更晚）不是最老一条 → 重置锚定在行 A + 5h
        assert_eq!(entry.windows[0].resets_at, Some(1_704_270_000_000 + FIVE_HOURS_MS));
    }

    /// 登录检测：auth.json 顶层 opencode-go.key 非空才有效；
    /// 文件缺失 / 解析失败 / 键缺失 / 空 key 一律视为未登录。
    #[test]
    fn go_login_detection() {
        let tmp = std::env::temp_dir().join(format!("zbar-oc-test-{}", std::process::id()));
        std::fs::create_dir_all(&tmp).expect("创建临时目录失败");
        let auth = tmp.join("auth.json");

        // 文件缺失 → false
        assert!(!has_go_login(&tmp));

        // 非空 key → true
        std::fs::write(&auth, r#"{"opencode-go":{"key":"sk-abc"}}"#).unwrap();
        assert!(has_go_login(&tmp));

        // 空 key / 键缺失 / 顶层无 opencode-go / 坏 JSON → false
        std::fs::write(&auth, r#"{"opencode-go":{"key":"  "}}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, r#"{"opencode-go":{}}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, r#"{"other":1}"#).unwrap();
        assert!(!has_go_login(&tmp));
        std::fs::write(&auth, "not json").unwrap();
        assert!(!has_go_login(&tmp));

        std::fs::remove_dir_all(&tmp).ok();
    }

    /// 日历窗口边界：周一 00:00 起点、周日起点回退到上周一、跨月重置。
    #[test]
    fn calendar_bounds() {
        // 周一 00:00 本身：周窗起点 = 自身，重置 = +7 天
        let b = window_bounds(MON_0);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.week_reset, MON_0 + WEEK_MS);
        assert_eq!(b.month_start, MON_0);
        assert_eq!(b.month_reset, FEB_1);

        // 周三正午：起点回退到本周一
        let b = window_bounds(WED_NOON);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.h5_start, WED_NOON - FIVE_HOURS_MS);

        // 周日（2024-01-07 00:00 = MON_0 + 6 天）：仍归本周
        let sunday = MON_0 + 6 * 86_400_000;
        let b = window_bounds(sunday);
        assert_eq!(b.week_start, MON_0);
        assert_eq!(b.week_reset, MON_0 + WEEK_MS);
    }
}

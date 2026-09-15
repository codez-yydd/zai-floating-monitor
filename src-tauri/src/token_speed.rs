//! Shared session-tree and request-speed primitives.
//!
//! The usage feed, the session HUD, and their two front ends must agree on
//! both the set of sessions being counted and the meaning of a speed value.
//! This module is deliberately independent of either renderer so that a
//! rollout hint can never turn into a confirmed token count and so that a
//! short request is not silently divided by an arbitrary one-second floor.

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpeedQuality {
    Generation,
    RequestAverage,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct SpeedSnapshot {
    pub value: f64,
    pub quality: SpeedQuality,
    pub completed_at: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub request_id: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum SpeedState {
    Measuring,
    Recent,
    Unavailable,
}

/// 待处理用户消息（pu）的新鲜期：一条尚无完成轮的 user 消息只在这段
/// 时间内可以独立支撑"等待首请求"的状态；超期后除非该会话树内还有
/// 可复核的活跃证据（进行中轮等），否则按空闲/不可用处理，防止数小时
/// 前的孤儿消息永久冒充活跃轮。取 10 分钟与 usage_feed::RUN_WINDOW_MS /
/// TOOL_WINDOW_MS 的"无活动即不再认为运行"口径一致；这绝不是给所有轮
/// 加硬截止——真实耗时很长的首请求只要会话树内仍有进行中请求（runs）
/// 就继续保留等待。
pub(crate) const PENDING_FRESH_MS: i64 = 10 * 60 * 1000;

/// `model_usage` is written as a completed-request table in older schemas,
/// where `status` is absent. Empty/null status has the same compatibility
/// meaning; explicit non-terminal statuses must not produce a speed.
pub(crate) fn is_completed_status(status: Option<&str>) -> bool {
    status
        .map(|value| value.is_empty() || value == "completed" || value == "success")
        .unwrap_or(true)
}

/// Timing fields for one completed model request.
///
/// `completed_at` is the actual completion timestamp when the source has
/// one. `duration_ms` is only an explicitly approximate fallback; it is never
/// used to manufacture a generation speed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub(crate) struct RequestTiming {
    pub output_tokens: i64,
    pub started_at: Option<i64>,
    pub first_token_at: Option<i64>,
    pub completed_at: Option<i64>,
    pub duration_ms: Option<i64>,
    pub request_id: Option<String>,
}

/// Compute a speed without applying a minimum duration or a stale-value
/// fallback. This form is useful for already validated historical rows.
pub(crate) fn request_speed(request: &RequestTiming) -> Option<SpeedSnapshot> {
    request_speed_at(request, 0)
}

/// Compute a speed and reject timestamps that are in the future relative to
/// `observed_at_ms` (when non-zero).
pub(crate) fn request_speed_at(
    request: &RequestTiming,
    observed_at_ms: i64,
) -> Option<SpeedSnapshot> {
    if request.output_tokens <= 0 {
        return None;
    }

    let started = request.started_at;
    if started.is_some_and(|value| value <= 0) {
        return None;
    }
    let first = request.first_token_at;
    if first.is_some_and(|value| value <= 0) {
        return None;
    }
    let completed = request.completed_at;
    if completed.is_some_and(|value| value <= 0) {
        return None;
    }

    if let Some(observed) = (observed_at_ms > 0).then_some(observed_at_ms) {
        if first.is_some_and(|value| value > observed)
            || completed.is_some_and(|value| value > observed)
        {
            return None;
        }
    }

    if let Some(started) = started {
        if first.is_some_and(|value| value < started)
            || completed.is_some_and(|value| value <= started)
        {
            return None;
        }
    }

    // A real generation speed needs the first-token and completion times.
    // `started_at` is useful for validating the row when present, but is not
    // required because a few schema versions only recorded the two absolute
    // events.
    if let (Some(first), Some(completed)) = (first, completed) {
        if completed > first {
            let span = completed - first;
            if let Some(value) = finite_rate(request.output_tokens, span) {
                return Some(SpeedSnapshot {
                    value,
                    quality: SpeedQuality::Generation,
                    completed_at: completed,
                    request_id: request.request_id.clone(),
                });
            }
        }
        // Both event timestamps were supplied but are reversed: this row is
        // invalid even if a duration fallback happens to be present.
        if completed <= first {
            return None;
        }
    }

    // A first-token timestamp without a completion timestamp is incomplete;
    // do not silently turn it into an average.
    if first.is_some() && completed.is_none() {
        return None;
    }

    // No first-token timestamp: a request-average speed is allowed. Prefer
    // the real absolute request span when both endpoints are present; use an
    // explicit duration only when one endpoint is unavailable.
    let duration = started
        .zip(completed)
        .and_then(|(started, completed)| completed.checked_sub(started))
        .filter(|value| *value > 0)
        .or_else(|| request.duration_ms.filter(|value| *value > 0))?;
    if let (Some(started), Some(completed)) = (started, completed) {
        if completed <= started {
            return None;
        }
    }

    let effective_completed = match (started, completed) {
        (_, Some(completed)) => completed,
        (Some(started), None) => started.checked_add(duration)?,
        // A polling timestamp is only an observation boundary. It is not a
        // completion event and must not manufacture a speed for a row that
        // has neither an absolute completion time nor a start time.
        (None, None) => return None,
    };
    if observed_at_ms > 0 && effective_completed > observed_at_ms {
        return None;
    }
    let value = finite_rate(request.output_tokens, duration)?;
    Some(SpeedSnapshot {
        value,
        quality: SpeedQuality::RequestAverage,
        completed_at: effective_completed,
        request_id: request.request_id.clone(),
    })
}

fn finite_rate(output_tokens: i64, span_ms: i64) -> Option<f64> {
    if span_ms <= 0 {
        return None;
    }
    let value = output_tokens as f64 * 1000.0 / span_ms as f64;
    value.is_finite().then_some(value).filter(|value| *value > 0.0)
}

/// 一条通过全部行级有效性判定的"真实生成样本"（供速度排行、汇总统计
/// 与项目会话聚合共用同一判定，杜绝各处口径漂移）：
/// - `output_tokens > 0`，完成状态由调用方按 `is_completed_status` 判定；
/// - `completed_at > first_token_at > 0`（真实事件时间区间）；
/// - 有 `started_at` 时要求 `first_token_at >= started_at` 且
///   `completed_at > started_at`（时刻与请求起点不能倒置）；
/// - `observed_at_ms > 0` 时拒绝未来时刻。
/// 合法短样本（<100ms）与高速样本（>500 t/s）不会仅因阈值被丢弃；首
/// Token 缺失的行在此口径下没有生成样本（只能走显式 request_average），
/// 逆序/未来/未完成/零输出一律返回 None。
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct GenerationSample {
    /// 合格输出 token 数（可加总分子）
    pub output_tokens: i64,
    /// 生成毫秒 = completed_at − first_token_at（可加总分母）
    pub generation_ms: i64,
    /// 该单笔请求的生成速度 t/s
    pub tps: f64,
}

/// 共享行级有效性判定：完整复用 `request_speed_at` 的校验路径，并额外
/// 要求结果是 `generation` 质量（排除 request_average 回退样本）。
pub(crate) fn generation_sample(
    request: &RequestTiming,
    observed_at_ms: i64,
) -> Option<GenerationSample> {
    let snapshot = request_speed_at(request, observed_at_ms)?;
    if snapshot.quality != SpeedQuality::Generation {
        return None;
    }
    let first = request.first_token_at.filter(|value| *value > 0)?;
    let completed = request.completed_at.filter(|value| *value > 0)?;
    let generation_ms = completed
        .checked_sub(first)
        .filter(|span| *span > 0)?;
    Some(GenerationSample {
        output_tokens: request.output_tokens,
        generation_ms,
        tps: snapshot.value,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SessionTree {
    pub root: String,
    pub members: Vec<String>,
}

#[derive(Debug, Clone, Default)]
pub(crate) struct SessionTreeIndex {
    has_parent: bool,
    parent_of: BTreeMap<String, String>,
    children: BTreeMap<String, Vec<String>>,
}

impl SessionTreeIndex {
    pub(crate) fn empty() -> Self {
        Self::default()
    }

    pub(crate) fn has_parent(&self) -> bool {
        self.has_parent
    }

    pub(crate) fn root_for(&self, session_id: &str) -> String {
        if !self.has_parent {
            return session_id.to_string();
        }

        let mut path = Vec::new();
        let mut position = BTreeMap::<String, usize>::new();
        let mut current = session_id.to_string();
        loop {
            if let Some(&start) = position.get(&current) {
                // A malformed cycle has no natural root. Choose a stable
                // canonical member so every entry point reports the same
                // tree instead of looping or double-counting unpredictably.
                return path[start..]
                    .iter()
                    .min()
                    .cloned()
                    .unwrap_or(current);
            }
            position.insert(current.clone(), path.len());
            path.push(current.clone());
            let Some(parent) = self.parent_of.get(&current) else {
                return current;
            };
            current = parent.clone();
        }
    }

    pub(crate) fn members(&self, root: &str) -> Vec<String> {
        let mut members = Vec::new();
        let mut seen = BTreeSet::new();
        let mut queue = VecDeque::from([root.to_string()]);
        while let Some(current) = queue.pop_front() {
            if !seen.insert(current.clone()) {
                continue;
            }
            members.push(current.clone());
            if let Some(children) = self.children.get(&current) {
                queue.extend(children.iter().cloned());
            }
        }
        members
    }

    pub(crate) fn tree(&self, root: &str) -> SessionTree {
        SessionTree {
            root: root.to_string(),
            members: self.members(root),
        }
    }

    #[cfg(test)]
    fn from_relations(relations: &[(&str, Option<&str>)]) -> Self {
        let mut index = Self {
            has_parent: true,
            ..Self::default()
        };
        for (id, parent) in relations {
            if let Some(parent) = parent.filter(|value| !value.is_empty()) {
                index
                    .parent_of
                    .insert((*id).to_string(), (*parent).to_string());
                index
                    .children
                    .entry((*parent).to_string())
                    .or_default()
                    .push((*id).to_string());
            }
        }
        index
    }
}

/// Load the complete session relation once. Missing tables/columns deliberately
/// return an empty legacy index, which means every requested session is its own
/// tree and preserves old-database behavior.
pub(crate) fn load_session_tree_index(conn: &Connection) -> Result<SessionTreeIndex, String> {
    let has_session = table_exists(conn, "session")
        && crate::db::has_column(conn, "session", "id")
        && crate::db::has_column(conn, "session", "parent_id");
    if !has_session {
        return Ok(SessionTreeIndex::empty());
    }

    let mut index = SessionTreeIndex {
        has_parent: true,
        ..SessionTreeIndex::default()
    };
    let mut stmt = conn
        .prepare("SELECT id, parent_id FROM session")
        .map_err(|error| format!("准备会话关系查询失败: {error}"))?;
    let rows = stmt
        .query_map([], |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        })
        .map_err(|error| format!("读取会话关系失败: {error}"))?;
    for row in rows {
        let (Some(id), parent) = row.map_err(|error| format!("读取会话关系失败: {error}"))? else {
            continue;
        };
        if id.is_empty() {
            continue;
        }
        if let Some(parent) = parent.filter(|value| !value.is_empty()) {
            index.parent_of.insert(id.clone(), parent.clone());
            index.children.entry(parent).or_default().push(id);
        }
    }
    for children in index.children.values_mut() {
        children.sort();
        children.dedup();
    }
    Ok(index)
}

fn table_exists(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT 1 FROM sqlite_master WHERE type = 'table' AND name = ?1 LIMIT 1",
        [table],
        |_| Ok(()),
    )
    .is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generation_speed_uses_exact_short_span_without_floor() {
        let request = RequestTiming {
            output_tokens: 50,
            started_at: Some(1_000),
            first_token_at: Some(1_010),
            completed_at: Some(1_020),
            ..RequestTiming::default()
        };
        let speed = request_speed(&request).expect("valid generation speed");
        assert_eq!(speed.quality, SpeedQuality::Generation);
        assert_eq!(speed.completed_at, 1_020);
        assert!((speed.value - 5_000.0).abs() < f64::EPSILON);
    }

    #[test]
    fn request_average_is_explicitly_approximate() {
        let request = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            duration_ms: Some(200),
            ..RequestTiming::default()
        };
        let speed = request_speed_at(&request, 2_500).expect("valid average speed");
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert_eq!(speed.completed_at, 2_200);
        assert!((speed.value - 500.0).abs() < f64::EPSILON);

        let from_absolute_request_times = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            completed_at: Some(2_200),
            ..RequestTiming::default()
        };
        let speed = request_speed(&from_absolute_request_times).expect("derived request duration");
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert_eq!(speed.completed_at, 2_200);
        assert!((speed.value - 500.0).abs() < f64::EPSILON);

        let absolute_times_override_conflicting_duration = RequestTiming {
            output_tokens: 100,
            started_at: Some(2_000),
            completed_at: Some(2_200),
            duration_ms: Some(50),
            ..RequestTiming::default()
        };
        let speed = request_speed(&absolute_times_override_conflicting_duration)
            .expect("absolute request span remains authoritative");
        assert!((speed.value - 500.0).abs() < f64::EPSILON);
    }

    #[test]
    fn invalid_timing_never_becomes_zero_or_stale_speed() {
        let reversed = RequestTiming {
            output_tokens: 100,
            first_token_at: Some(3_000),
            completed_at: Some(2_999),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed(&reversed).is_none());

        let future = RequestTiming {
            output_tokens: 100,
            started_at: Some(4_000),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&future, 4_050).is_none());

        let zero = RequestTiming {
            output_tokens: 0,
            ..RequestTiming::default()
        };
        assert!(request_speed(&zero).is_none());

        let first_before_start = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(4_999),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(request_speed(&first_before_start).is_none());

        let first_without_completion = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(5_100),
            duration_ms: Some(900),
            ..RequestTiming::default()
        };
        assert!(request_speed(&first_without_completion).is_none());

        let duration_without_timestamps = RequestTiming {
            output_tokens: 100,
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&duration_without_timestamps, 9_999).is_none());
    }

    #[test]
    fn generation_sample_accepts_short_and_fast_but_rejects_invalid_rows() {
        // 真实输出 1000 Token、首 Token 到完成 8 秒：可信 125 t/s——TTFT 再长
        // （92 秒）也不改用旧"总耗时−TTFT/兜底"公式。
        let long_ttft = RequestTiming {
            output_tokens: 1000,
            started_at: Some(1_000),
            first_token_at: Some(93_000),
            completed_at: Some(101_000),
            ..RequestTiming::default()
        };
        let sample = generation_sample(&long_ttft, 200_000).expect("长 TTFT 样本仍可信");
        assert_eq!(sample.generation_ms, 8_000);
        assert!((sample.tps - 125.0).abs() < f64::EPSILON);

        // 合法 <100ms 与 >500 t/s 的行不能仅因阈值被丢弃（新口径无这些阈值）。
        let short = RequestTiming {
            output_tokens: 50,
            started_at: Some(1_000),
            first_token_at: Some(1_010),
            completed_at: Some(1_020),
            ..RequestTiming::default()
        };
        let sample = generation_sample(&short, 2_000).expect("10ms 合法短样本");
        assert!((sample.tps - 5_000.0).abs() < f64::EPSILON);

        // 逆序（completed <= first）：无样本。
        let reversed = RequestTiming {
            output_tokens: 100,
            first_token_at: Some(3_000),
            completed_at: Some(2_999),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&reversed, 9_999).is_none());

        // 未来时刻（晚于观测时刻）：无样本。
        let future = RequestTiming {
            output_tokens: 100,
            started_at: Some(4_000),
            first_token_at: Some(4_100),
            completed_at: Some(4_200),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&future, 4_100).is_none());

        // first 早于 started：无样本。
        let before_start = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            first_token_at: Some(4_999),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&before_start, 9_999).is_none());

        // 零输出：无样本。
        let zero_output = RequestTiming {
            output_tokens: 0,
            started_at: Some(5_000),
            first_token_at: Some(5_100),
            completed_at: Some(6_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&zero_output, 9_999).is_none());

        // 首 Token 缺失：只能算 request_average，绝不算生成样本。
        let no_first = RequestTiming {
            output_tokens: 100,
            started_at: Some(5_000),
            completed_at: Some(6_000),
            duration_ms: Some(1_000),
            ..RequestTiming::default()
        };
        assert!(generation_sample(&no_first, 9_999).is_none());
        assert_eq!(
            request_speed_at(&no_first, 9_999).map(|speed| speed.quality),
            Some(SpeedQuality::RequestAverage)
        );
    }

    #[test]
    fn deep_tree_and_cycle_are_finite_and_stable() {
        let index = SessionTreeIndex::from_relations(&[
            ("root", None),
            ("a", Some("root")),
            ("b", Some("a")),
            ("c", Some("b")),
        ]);
        assert_eq!(index.root_for("c"), "root");
        assert_eq!(index.members("root"), ["root", "a", "b", "c"]);

        let cycle = SessionTreeIndex::from_relations(&[
            ("z", Some("y")),
            ("y", Some("x")),
            ("x", Some("y")),
        ]);
        assert_eq!(cycle.root_for("x"), "x");
        assert_eq!(cycle.root_for("y"), "x");
        assert_eq!(cycle.members("x"), ["x", "y", "z"]);
    }

    #[test]
    fn legacy_index_keeps_each_session_separate() {
        let index = SessionTreeIndex::empty();
        assert!(!index.has_parent());
        assert_eq!(index.root_for("child"), "child");
        assert_eq!(index.members("child"), ["child"]);
    }
}

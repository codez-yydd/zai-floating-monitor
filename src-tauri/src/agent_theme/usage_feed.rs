//! 对话页用量统计条的数据源：皮肤已安装时后台轮询 ZCode 主库的
//! `turn_usage` 表（官方每轮聚合表，轮次完成时才落库），把最近 7 天内
//! （至多 3000 轮，超出保留最新）的轮次用量序列化为 `usage-data.js`
//! 写出到主题目录，供注入的 usage.js 周期加载并在对话区每轮下方渲染
//! 统计条。
//!
//! 数据契约（usage-data.js 内容，键名与 inject::USAGE_JS 消费端一字不差）：
//! ```js
//! window.__ZBAR_USAGE__ = { v: 2, ts: <最后数据变化时刻ms>, turns: [{
//!   umid: "msg_xxx",   用户消息 id（turn_usage.user_message_id，实测与
//!                      ZCode DOM 的 data-turn-id 同值，即渲染端匹配键；
//!                      列缺失或值为 null 时输出 null，该轮无法被匹配）
//!   turn: "turn_xxx",  轮 id（保留透出，不用于 DOM 匹配）
//!   sess: "sess_xxx",  status: "completed",
//!   start / end: 起止毫秒,
//!   in / out / cr / cw / rt: 输入(含缓存读) / 输出 / 缓存读 / 缓存写 / 推理，
//!     —— 均已并入该轮覆盖到的子代理聚合，
//!   req / retry / tool: 模型请求数 / 重试数 / 工具调用数（同样已并入），
//!   dur / ttft: 主轮自身总耗时 / 首字延迟毫秒（可能 null），
//!   sub: { n, req, in, out, cr, cw, rt } 并入的子代理聚合（可能 null），
//!   models: "GLM-5.3,..."  该轮用到的模型（去重逗号拼接，含子代理），
//! }],
//!   runs: [{              进行中轮实时聚合（v2 格式不变的附加字段，旧
//!                         渲染脚本忽略未知字段，平滑兼容；空数组也输出）
//!   umid: "msg_xxx"|null  用户消息 id（model_usage.parent_user_message_id，
//!                         实测与 DOM data-turn-id 同值；子代理轮指向子代理
//!                         会话自己的消息，主会话 DOM 匹配不到，仅保留数据）
//!   sess: "sess_xxx",     会话 id（子代理进行中轮为 sess_subagent_* 形态）
//!   psess: "sess_xxx"|null 父会话 id（仅子代理会话查 session.parent_id，
//!                         主会话为 null；渲染端据此并入父会话累计）
//!   m: 1                  仅子代理行且父会话存在进行中主轮行时输出：
//!                         数值已并入该主轮行 sub，渲染端会话累计跳过
//!                         本行防双计（其余行无此键）
//!   sub: {n,req,in,out,cr,cw,rt}
//!                         仅主会话行输出：并入本主轮的子代理实时聚合
//!                         （子代理 runs 行按 psess 归并 + 游离子代理
//!                         完成轮，见下方 runs 侧并入策略；无并入则
//!                         无此键）
//!   in / out / cr / cw / rt: 该轮已完成模型请求的 token 合计（每完成一步
//!                         请求 model_usage 即落一行，2 秒轮询内可见）
//!   req: 行数,            已完成的模型请求数
//!   start: 首个请求开始毫秒
//! }] }
//! ```
//!
//! ## runs（进行中轮）口径（实库验证结论）
//!
//! `model_usage` 每次模型请求完成即落一行（无 running 行），而
//! `turn_usage` 整轮结束才写：进行中的轮 = model_usage 有该 turn_id 的行
//! 且 turn_usage 尚无该 turn_id 的行。runs 取近 10 分钟窗口内按 turn_id
//! 分组的 model_usage 聚合，过滤掉已完成（done）轮；窗口放宽为"组内任一
//! 请求落在近 10 分钟 + 组行扫近 7 天"，覆盖长轮（>10 分钟）的完整聚合。
//! 轮完成后 turn_usage 行出现 → 该 turn_id 进入 done 集合 → runs 行消失、
//! turn_usage 行进入 turns（渲染端最终值无缝接管）。
//!
//! ## 子代理并入策略（实库验证结论，v3.10.1 主库实测）
//!
//! 子代理会话（session_id 形如 `sess_subagent_agent_<uuid>`）的轮次不单独
//! 显示，而是并入父会话中时间覆盖它的主轮（主轮 started_at ≤ 子轮
//! started_at 且 子轮 completed_at ≤ 主轮 completed_at；父会话经
//! session.parent_id 查得）。曾考虑的"精确关联"方案（子代理
//! model_usage.parent_user_message_id 指向父会话消息）经实库验证**不成立**：
//! 该字段指向的是子代理会话自己的消息（JOIN message 后 msg_session =
//! 子代理 session_id），无法回溯父会话，故采用时间窗口法。
//! 实测覆盖率 64/69（93%）：未命中的子轮均为合法边界——父会话轮尚未
//! 完成落库（随后续导出周期自然并入，全量重查 7 天窗口自带此自愈性）、
//! 父会话无 turn_usage 行（旧版本库）、子轮完成晚于父轮完成（后台代理
//! 越界运行）。未命中的子轮直接丢弃，不单独显示。
//!
//! ## runs 侧子代理并入（V9：主轮实时条与会话累计条实时反映子代理消耗）
//!
//! turns 的整轮并入只在主轮完成落库后发生；主轮进行期间需要把子代理
//! 消耗实时反映到主轮 runs 行上。口径：
//! - 每条主会话 runs 行新增 sub 聚合 = 所有 psess 指向该会话的子代理
//!   runs 行（一次会话并行多个子代理时全部并入）+ 游离子代理完成轮
//!   （子代理 turn_usage 行已落库、但所属主轮尚未落库而未被
//!   merge_subagent_turns 并入 turns 的部分；防双计判定沿用 merge 的
//!   时间窗口匹配逻辑：父会话存在"started_at ≤ 子轮 started_at <
//!   completed_at"的完成主轮 = 所属主轮已落库（子轮越界完成等边界），
//!   维持丢弃不输出；否则作为游离子代理完成轮输出给 runs 侧聚合）；
//! - 对应地，父会话存在进行中主轮行（同批 runs 内）的子代理 runs 行
//!   打 m:1 标记（其数值已并入主轮行 sub，渲染端会话累计跳过本行防
//!   双计）；父会话暂无主轮行（主轮首笔请求未完成）时不打标，渲染端
//!   按 psess 直接并入会话累计，主轮行出现后自动切换口径，无缝衔接；
//! - 子代理会话自己的 DOM（详情面板，同 document）按子代理行 umid
//!   匹配渲染自身统计，与主轮并入互不影响（同一数值两处展示是预期：
//!   主轮行是"并入视图"，子代理行是"自身视图"；会话累计只算一次）。
//!
//! ## 降级与健壮性
//!
//! - 老版本 ZCode 库可能没有 turn_usage 表或部分列：沿用 db::has_column
//!   的探测降级模式，表/核心列缺失时整个功能静默关闭（不导出不显示），
//!   非核心列缺失按 0 / NULL 降级；
//! - 主库只读连接（复用 zcode_sessions::open_main_db_readonly_uri），
//!   查询失败静默跳过本轮（下个周期重试），不 panic 不刷日志；
//! - 增量策略：每 2 秒全量重查最近 7 天窗口（至多 3000 轮、超出保留
//!   最新，防数据文件无限膨胀；长窗口覆盖打开旧会话的历史回填，优先
//!   简单方案，且天然覆盖"主轮晚于子轮落库"的并期场景）；序列化字节
//!   无变化则跳过写盘，写盘走 .tmp + rename 原子替换。

use crate::agent_theme::store;
use rusqlite::Connection;
use serde::Serialize;
use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::thread;
use std::time::Duration;

/// 当前唯一支持的目标应用（与 mod.rs 注册表一致；feed 挂载点均由
/// agent_theme 的安装/卸载流程驱动，实际 app_id 恒为 zcode）
const TARGET_APP_ID: &str = "zcode";

/// 导出窗口：只导出最近 7 天的轮次（打开旧会话时历史轮普遍超过原 6 小时
/// 窗口导致统计条无数据，长窗口覆盖历史回填场景；更早的历史由 ZBar 面板
/// 统计覆盖）
const WINDOW_MS: i64 = 7 * 24 * 3600 * 1000;

/// 导出行数上限：窗口内轮次超过 3000 时仅保留最新 3000 轮（配合长窗口
/// 防数据文件无限膨胀；截断按 started_at 升序取末尾即最新的轮次）
const MAX_TURNS: usize = 3000;

/// runs（进行中轮）新鲜度窗口：该轮最新请求落在本窗口内才视为进行中
/// （防陈旧——异常中断的轮 model_usage 行不再增长，10 分钟后自动退出
/// runs 通道；正常轮远快于此，轮完成即由 done 集合过滤）
const RUN_WINDOW_MS: i64 = 10 * 60 * 1000;

/// 导出周期（毫秒）：与注入端 usage.js 的数据重载周期一致
const INTERVAL_MS: u64 = 2000;

// ============================================================
// 导出数据结构（JSON 键名即 usage-data.js 契约，勿改）
// ============================================================

/// 导出的单轮用量。字段语义见模块头的数据契约。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct UsageTurn {
    /// 轮 id（turn_usage.turn_id，保留透出；DOM data-turn-id 实测并非
    /// 此值，渲染端匹配不使用本字段）
    #[serde(rename = "turn")]
    turn_id: String,
    /// 用户消息 id（turn_usage.user_message_id，实测与 ZCode DOM 的
    /// data-turn-id 同值同源，渲染端匹配键；列缺失或值为 null 时导出
    /// null——该轮无法被 DOM 匹配，仅保留数据）
    #[serde(rename = "umid")]
    user_message_id: Option<String>,
    /// 会话 id
    #[serde(rename = "sess")]
    session_id: String,
    /// completed / cancelled / error 等（原样透出）
    status: String,
    /// 开始时刻（毫秒）
    start: i64,
    /// 完成时刻（毫秒，缺失为 null）
    end: Option<i64>,
    /// 输入 token（turn_usage 原始值，含缓存读；前端展示 ↑ = in − cr）
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 输出 token
    #[serde(rename = "out")]
    output_tokens: i64,
    /// 缓存读 token
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 缓存写 token
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 推理 token
    #[serde(rename = "rt")]
    reasoning: i64,
    /// 模型请求数
    #[serde(rename = "req")]
    requests: i64,
    /// 模型重试数
    #[serde(rename = "retry")]
    retries: i64,
    /// 工具调用数
    #[serde(rename = "tool")]
    tool_calls: i64,
    /// 总耗时毫秒（主轮自身，不随并入变化；缺失为 null）
    dur: Option<i64>,
    /// 首字延迟毫秒（主轮自身，不随并入变化；缺失为 null）
    ttft: Option<i64>,
    /// 并入的子代理聚合（无并入为 null）
    sub: Option<SubAgg>,
    /// 该轮用到的模型（去重逗号拼接，含并入子代理的模型）
    models: String,
}

/// 子代理并入聚合明细（usage.js hover 展示用）
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct SubAgg {
    /// 并入的子代理轮数
    n: i64,
    /// 子代理模型请求数合计
    #[serde(rename = "req")]
    requests: i64,
    /// 子代理输入 token 合计
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 子代理输出 token 合计
    #[serde(rename = "out")]
    output_tokens: i64,
    /// 子代理缓存读 token 合计
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 子代理缓存写 token 合计
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 子代理推理 token 合计
    #[serde(rename = "rt")]
    reasoning: i64,
}

impl SubAgg {
    fn empty() -> Self {
        SubAgg {
            n: 0,
            requests: 0,
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
        }
    }

    /// 累入一条子代理轮（n 计 1 轮，其余逐项累加；turns 整轮并入与
    /// runs 实时并入共用同一口径）
    fn add(
        &mut self,
        requests: i64,
        input_tokens: i64,
        output_tokens: i64,
        cache_read: i64,
        cache_write: i64,
        reasoning: i64,
    ) {
        self.n += 1;
        self.requests += requests;
        self.input_tokens += input_tokens;
        self.output_tokens += output_tokens;
        self.cache_read += cache_read;
        self.cache_write += cache_write;
        self.reasoning += reasoning;
    }
}

/// 从库中读出的子代理轮原始行（并入聚合前的中间形态）
#[derive(Debug, Clone)]
struct SubTurnRow {
    turn_id: String,
    /// 父会话 id（session.parent_id）
    parent_session_id: Option<String>,
    started_at: i64,
    completed_at: Option<i64>,
    input_tokens: i64,
    output_tokens: i64,
    cache_read: i64,
    cache_write: i64,
    reasoning: i64,
    requests: i64,
    retries: i64,
    tool_calls: i64,
}

/// 进行中轮的实时聚合（runs 数组元素）。字段语义见模块头的数据契约；
/// JSON 键名即 usage.js 消费端契约，勿改。与 UsageTurn 的差异：无
/// status/dur/ttft/models（进行中轮无整轮聚合可读），多 psess（子代理
/// 进行中轮并入父会话累计的关联键）与 m（已并入主轮行 sub 的防双计
/// 标记）；sub 为 runs 侧实时并入的子代理聚合（结构与 UsageTurn.sub
/// 一致，仅主会话行携带）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub(crate) struct UsageRun {
    /// 用户消息 id（model_usage.parent_user_message_id，实测与 DOM
    /// data-turn-id 同值，渲染端匹配键；子代理轮指向子代理会话自己的
    /// 消息，主会话 DOM 匹配不到；列缺失或值为 null 时导出 null——
    /// 该轮无法与 DOM 匹配，仅保留数据并入会话累计）
    #[serde(rename = "umid")]
    user_message_id: Option<String>,
    /// 会话 id（子代理进行中轮为 sess_subagent_* 形态）
    #[serde(rename = "sess")]
    session_id: String,
    /// 父会话 id（仅子代理会话查 session.parent_id 得出，主会话为 null；
    /// 渲染端按 sess 或 psess 命中当前会话并入累计）
    #[serde(rename = "psess")]
    parent_session_id: Option<String>,
    /// 输入 token 合计（该轮已完成模型请求，含缓存读；展示 ↑ = in − cr）
    #[serde(rename = "in")]
    input_tokens: i64,
    /// 输出 token 合计
    #[serde(rename = "out")]
    output_tokens: i64,
    /// 缓存读 token 合计
    #[serde(rename = "cr")]
    cache_read: i64,
    /// 缓存写 token 合计
    #[serde(rename = "cw")]
    cache_write: i64,
    /// 推理 token 合计
    #[serde(rename = "rt")]
    reasoning: i64,
    /// 模型请求数（model_usage 行数）
    #[serde(rename = "req")]
    requests: i64,
    /// 本轮首个（扫查窗口内）请求开始时刻（毫秒）
    start: i64,
    /// 已并入父会话主轮行 sub 的标记（1 = 本行数值已并入对应主轮行
    /// sub，渲染端会话累计跳过本行防双计）。仅子代理行且父会话存在
    /// 进行中主轮行时置位；None 不序列化（主会话行与无主轮行的子代理
    /// 行均无此键）
    #[serde(rename = "m", skip_serializing_if = "Option::is_none")]
    merged: Option<u8>,
    /// 并入本主轮行的子代理实时聚合（仅主会话行：同批子代理 runs 行
    /// 按 psess 归并 + 游离子代理完成轮；None 不序列化）
    #[serde(skip_serializing_if = "Option::is_none")]
    sub: Option<SubAgg>,
}

// ============================================================
// 后台任务（普通 thread + flag 模式，沿用项目 spawn_sync_worker 惯例）
// ============================================================

/// 停止标记：卸载/还原皮肤时置位；线程完成当前导出周期（含 DB busy
/// 等待，最长约 3 秒余）后，在下一个检查点退出
static FEED_STOP: AtomicBool = AtomicBool::new(false);
/// 导出线程句柄（含已退出的旧句柄，start 时检测并复用槽位，防重复起线程）
static FEED_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();

fn feed_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    FEED_HANDLE.get_or_init(|| Mutex::new(None))
}

/// 应用启动挂点：皮肤已安装时启动导出线程（未安装不启动，零开销）。
pub fn start_if_installed() {
    if store::load_state(TARGET_APP_ID).is_installed() {
        start();
    }
}

/// 启动导出线程（安装成功挂点调用）。已在运行时为幂等 no-op（只清掉可能
/// 残留的停止标记，覆盖 stop 后线程未退完又立即 start 的窄窗口）。
pub fn start() {
    let mut guard = match feed_handle().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_ref().is_some_and(|h| !h.is_finished()) {
        FEED_STOP.store(false, Ordering::Relaxed);
        return;
    }
    FEED_STOP.store(false, Ordering::Relaxed);
    // 启动失败仅放弃本功能（不 panic 不阻塞调用方），后续挂点可重试
    if let Ok(h) = thread::Builder::new()
        .name("zbar-usage-feed".into())
        .spawn(feed_loop)
    {
        *guard = Some(h);
    }
}

/// 停止导出线程（卸载/还原成功挂点调用）。仅置位停止标记，不 join 不等待；
/// 线程完成当前导出周期（含可能的 DB busy 等待，最长约 3 秒余）后，
/// 于下一个检查点退出。
pub fn stop() {
    FEED_STOP.store(true, Ordering::Relaxed);
}

fn feed_loop() {
    // 变化检测缓存：线程生命周期内持有上轮 turns 序列化字节（线程重启
    // 丢失缓存只多写一次盘，无正确性影响）
    let mut cache: Option<String> = None;
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        // 每轮再核对安装状态：皮肤被异常还原（state 复位而未走 stop 挂点）
        // 时自动退出，不留空转线程
        if !store::load_state(TARGET_APP_ID).is_installed() {
            return;
        }
        export_once(&mut cache);
        // 分段睡眠：sleep 期间可及时感知 stop（export_once 期间不响应）。
        // stop 仅置位 flag、不 join 不等待：线程在完成当前导出周期（含可能
        // 的 DB busy 等待，最长约 3 秒余）后于下一个检查点退出（应用退出时
        // 进程随之结束，与项目其它后台线程同款不显式 join）
        for _ in 0..(INTERVAL_MS / 100) {
            if FEED_STOP.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 单轮导出：读库 → 序列化 → 变化检测后原子写出。任何失败静默跳过本轮
/// （下个周期重试），不 panic 不累积日志。
fn export_once(cache: &mut Option<String>) {
    let result = (|| -> Result<(), String> {
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        // turns + 游离子代理完成轮（turns/merge 阶段分流输出，见
        // merge_subagent_turns）；None = turn_usage 表/核心列缺失（老版本
        // ZCode），功能静默关闭
        let Some((turns, sub_orphans)) = collect_turns(&conn, now_ms - WINDOW_MS)? else {
            return Ok(());
        };
        // 进行中轮 runs：与 turns 同连接同轮询周期读出。刻意不做"失败降级
        // 空数组"——runs 与 turns 任一查询失败都整体跳过本轮（下周期重试），
        // 避免 runs 闪空导致渲染端实时段闪烁断档
        let done = collect_done_turn_ids(&conn, now_ms - WINDOW_MS)?;
        let runs = collect_runs(
            &conn,
            now_ms - RUN_WINDOW_MS,
            now_ms - WINDOW_MS,
            &done,
            &sub_orphans,
        )?;
        let turns_json = serde_json::to_string(&turns)
            .map_err(|e| format!("序列化用量数据失败: {e}"))?;
        let runs_json =
            serde_json::to_string(&runs).map_err(|e| format!("序列化进行中轮失败: {e}"))?;
        let dir = store::app_dir(TARGET_APP_ID)?;
        fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
        write_if_changed(&dir, cache, &turns_json, &runs_json, now_ms)?;
        Ok(())
    })();
    if result.is_err() {
        // 静默跳过本轮（库被锁超时、目录暂不可写等瞬态），下个周期重试；
        // 刻意不记日志避免刷屏
    }
}

// ============================================================
// 读库与聚合（纯逻辑拆分便于单元测试，不依赖真实 ~/.zcode）
// ============================================================

/// 探测表是否存在（table 为代码内常量，无注入风险）
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// 数值列探测降级表达式：列存在取 COALESCE(col, 0)，缺失取常量 0
/// （老版本 turn_usage 可能缺部分统计列，按现有 has_column 模式降级）
fn num_col(conn: &Connection, table: &str, col: &str) -> String {
    if crate::db::has_column(conn, table, col) {
        format!("COALESCE({col}, 0)")
    } else {
        "0".to_string()
    }
}

/// 可空列探测降级表达式：列存在取列名，缺失取 NULL
fn opt_col(conn: &Connection, table: &str, col: &str) -> String {
    if crate::db::has_column(conn, table, col) {
        col.to_string()
    } else {
        "NULL".to_string()
    }
}

/// 读出最近窗口内的主会话轮 + 并入子代理轮 + 模型清单，返回
/// (导出序列, 游离子代理完成轮)。游离子代理完成轮 = 子代理 turn_usage
/// 行已落库、但所属主轮尚未落库未被并入 turns 的部分（merge 阶段分流，
/// 见 merge_subagent_turns），交由 runs 侧聚合进主轮行 sub。
/// Ok(None) = 功能关闭（turn_usage 表或核心列缺失）；
/// Err = 瞬态查询失败（调用方静默跳过本轮）。
fn collect_turns(
    conn: &Connection,
    window_start_ms: i64,
) -> Result<Option<(Vec<UsageTurn>, Vec<SubTurnRow>)>, String> {
    // 功能总开关：表或任一核心列缺失 → 整个功能静默关闭（不导出不显示）
    if !has_table(conn, "turn_usage")
        || !crate::db::has_column(conn, "turn_usage", "session_id")
        || !crate::db::has_column(conn, "turn_usage", "turn_id")
        || !crate::db::has_column(conn, "turn_usage", "status")
        || !crate::db::has_column(conn, "turn_usage", "started_at")
    {
        return Ok(None);
    }

    // ---- 主会话轮（子代理会话整段排除，sub 前缀覆盖
    //      sess_subagent_agent_ 及将来可能的其它子代理形态）----
    let (inp, out, rt) = (
        num_col(conn, "turn_usage", "input_tokens"),
        num_col(conn, "turn_usage", "output_tokens"),
        num_col(conn, "turn_usage", "reasoning_tokens"),
    );
    let (cw, cr) = (
        num_col(conn, "turn_usage", "cache_creation_input_tokens"),
        num_col(conn, "turn_usage", "cache_read_input_tokens"),
    );
    let (req, retry, tool) = (
        num_col(conn, "turn_usage", "model_request_count"),
        num_col(conn, "turn_usage", "model_retry_count"),
        num_col(conn, "turn_usage", "tool_call_count"),
    );
    let completed = opt_col(conn, "turn_usage", "completed_at");
    let dur = opt_col(conn, "turn_usage", "duration_ms");
    let ttft = opt_col(conn, "turn_usage", "time_to_first_token_ms");
    // umid（用户消息 id）为非核心可空列：老版本缺列时整列降级 NULL
    let umid = opt_col(conn, "turn_usage", "user_message_id");
    let sql = format!(
        "SELECT session_id, turn_id, COALESCE(status, ''), started_at, {completed}, \
         {inp}, {out}, {rt}, {cw}, {cr}, {req}, {retry}, {tool}, {dur}, {ttft}, {umid} \
         FROM turn_usage \
         WHERE started_at >= ?1 AND session_id NOT LIKE 'sess_subagent%' \
         ORDER BY started_at ASC"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备 turn_usage 查询失败: {e}"))?;
    let mut turns: Vec<UsageTurn> = stmt
        .query_map([window_start_ms], |row| {
            Ok(UsageTurn {
                session_id: row.get(0)?,
                turn_id: row.get(1)?,
                status: row.get(2)?,
                start: row.get::<_, Option<i64>>(3)?.unwrap_or(0),
                end: row.get(4)?,
                input_tokens: row.get::<_, i64>(5)?.max(0),
                output_tokens: row.get::<_, i64>(6)?.max(0),
                reasoning: row.get::<_, i64>(7)?.max(0),
                cache_write: row.get::<_, i64>(8)?.max(0),
                cache_read: row.get::<_, i64>(9)?.max(0),
                requests: row.get::<_, i64>(10)?.max(0),
                retries: row.get::<_, i64>(11)?.max(0),
                tool_calls: row.get::<_, i64>(12)?.max(0),
                dur: row.get::<_, Option<i64>>(13)?,
                ttft: row.get::<_, Option<i64>>(14)?,
                user_message_id: row.get::<_, Option<String>>(15)?,
                sub: None,
                models: String::new(),
            })
        })
        .map_err(|e| format!("读取 turn_usage 失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取 turn_usage 失败: {e}"))?;
    turns.retain(|t| t.start > 0 && !t.turn_id.is_empty());
    // 行数上限：超出保留最新（序列按 started_at 升序，丢弃头部最旧的轮次）
    if turns.len() > MAX_TURNS {
        turns.drain(..turns.len() - MAX_TURNS);
    }

    // ---- 子代理轮（并入父会话中时间覆盖它的主轮，见模块头验证结论）----
    let mut subs: Vec<SubTurnRow> = Vec::new();
    // 时间窗口比对需要两侧 completed_at；session 表/parent_id 列缺失
    // （老版本）时放弃并入，子轮整体不显示
    if has_table(conn, "session")
        && crate::db::has_column(conn, "session", "parent_id")
        && crate::db::has_column(conn, "turn_usage", "completed_at")
    {
        let (s_inp, s_out, s_rt) = (
            num_col(conn, "turn_usage", "input_tokens"),
            num_col(conn, "turn_usage", "output_tokens"),
            num_col(conn, "turn_usage", "reasoning_tokens"),
        );
        let (s_cw, s_cr) = (
            num_col(conn, "turn_usage", "cache_creation_input_tokens"),
            num_col(conn, "turn_usage", "cache_read_input_tokens"),
        );
        let (s_req, s_retry, s_tool) = (
            num_col(conn, "turn_usage", "model_request_count"),
            num_col(conn, "turn_usage", "model_retry_count"),
            num_col(conn, "turn_usage", "tool_call_count"),
        );
        let sub_sql = format!(
            "SELECT tu.turn_id, s.parent_id, tu.started_at, tu.completed_at, \
             {s_inp}, {s_out}, {s_rt}, {s_cw}, {s_cr}, {s_req}, {s_retry}, {s_tool} \
             FROM turn_usage tu JOIN session s ON s.id = tu.session_id \
             WHERE tu.started_at >= ?1 AND tu.session_id LIKE 'sess_subagent%'"
        );
        let mut stmt = conn
            .prepare(&sub_sql)
            .map_err(|e| format!("准备子代理轮查询失败: {e}"))?;
        let rows = stmt
            .query_map([window_start_ms], |row| {
                Ok(SubTurnRow {
                    turn_id: row.get(0)?,
                    parent_session_id: row.get(1)?,
                    started_at: row.get::<_, Option<i64>>(2)?.unwrap_or(0),
                    completed_at: row.get(3)?,
                    input_tokens: row.get::<_, i64>(4)?.max(0),
                    output_tokens: row.get::<_, i64>(5)?.max(0),
                    reasoning: row.get::<_, i64>(6)?.max(0),
                    cache_write: row.get::<_, i64>(7)?.max(0),
                    cache_read: row.get::<_, i64>(8)?.max(0),
                    requests: row.get::<_, i64>(9)?.max(0),
                    retries: row.get::<_, i64>(10)?.max(0),
                    tool_calls: row.get::<_, i64>(11)?.max(0),
                })
            })
            .map_err(|e| format!("读取子代理轮失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取子代理轮失败: {e}"))?;
        subs.extend(rows);
    }
    let (merged_pairs, sub_orphans) = merge_subagent_turns(&mut turns, subs);

    // ---- 模型清单：该轮 model_usage 的去重 model_id（含并入子轮）----
    if has_table(conn, "model_usage")
        && crate::db::has_column(conn, "model_usage", "turn_id")
        && crate::db::has_column(conn, "model_usage", "model_id")
        && crate::db::has_column(conn, "model_usage", "started_at")
    {
        let mut stmt = conn
            .prepare(
                "SELECT turn_id, model_id FROM model_usage \
                 WHERE started_at >= ?1 AND model_id IS NOT NULL AND model_id != ''",
            )
            .map_err(|e| format!("准备 model_usage 查询失败: {e}"))?;
        let rows = stmt
            .query_map([window_start_ms], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })
            .map_err(|e| format!("读取 model_usage 失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取 model_usage 失败: {e}"))?;
        attach_models(&mut turns, &build_models_map(rows), &merged_pairs);
    }

    Ok(Some((turns, sub_orphans)))
}

/// 读出扫查窗口内已完成轮的 turn_id 集合（turn_usage 有行 = 整轮已结束）。
/// runs 据此过滤完成轮；窗口取完整 7 天而非 10 分钟——长轮（>10 分钟）的
/// turn_usage.started_at 早于 runs 新鲜度窗口，漏查会导致完成后最长 10 分钟
/// 内残留 runs 行（渲染端虽以 turn_usage 优先，但会话累计会双重计数）。
fn collect_done_turn_ids(
    conn: &Connection,
    sweep_start_ms: i64,
) -> Result<BTreeSet<String>, String> {
    let mut stmt = conn
        .prepare("SELECT DISTINCT turn_id FROM turn_usage WHERE started_at >= ?1")
        .map_err(|e| format!("准备完成轮查询失败: {e}"))?;
    let rows = stmt
        .query_map([sweep_start_ms], |row| row.get::<_, String>(0))
        .map_err(|e| format!("读取完成轮失败: {e}"))?
        .collect::<Result<BTreeSet<_>, _>>()
        .map_err(|e| format!("读取完成轮失败: {e}"))?;
    Ok(rows)
}

/// 聚合进行中轮（runs）：近 10 分钟有模型请求、且 turn_usage 尚无该
/// turn_id 行的轮，按 turn_id 分组输出 model_usage 合计。
/// - recent_start_ms = now − 10 分钟（新鲜度窗口）；sweep_start_ms =
///   now − 7 天（组行扫查下界，覆盖长轮早期请求的完整聚合）；
/// - sub_orphans = 游离子代理完成轮（collect_turns/merge 阶段分流输出，
///   所属主轮尚未落库的部分）：按 parent_session_id 并入对应主会话行
///   sub（与子代理 runs 行同口径聚合）；
/// - 每条主会话行 sub = 同批内 psess 指向它的子代理行 + 游离子代理
///   完成轮（多子代理并行全并）；父会话存在主会话行的子代理行打 m:1
///   （防会话累计双计，见模块头 runs 侧并入策略）；
/// - model_usage 表/核心列缺失（老版本库）→ Ok(空)，不影响 turns 导出；
/// - 行数量级：窗口内行数 = 10 分钟内完成的模型请求数（重度使用数百行），
///   分组后 runs 行数 = 活跃轮数（通常个位数），每 2 秒一次开销可忽略。
fn collect_runs(
    conn: &Connection,
    recent_start_ms: i64,
    sweep_start_ms: i64,
    done_turn_ids: &BTreeSet<String>,
    sub_orphans: &[SubTurnRow],
) -> Result<Vec<UsageRun>, String> {
    // 核心列缺失 → 无 runs（老版本库无 parent_user_message_id 等列时
    // 按下方 num_col/opt_col 逐列降级，不整体放弃）
    if !has_table(conn, "model_usage")
        || !crate::db::has_column(conn, "model_usage", "turn_id")
        || !crate::db::has_column(conn, "model_usage", "session_id")
        || !crate::db::has_column(conn, "model_usage", "started_at")
    {
        return Ok(Vec::new());
    }
    let (inp, out, rt) = (
        num_col(conn, "model_usage", "input_tokens"),
        num_col(conn, "model_usage", "output_tokens"),
        num_col(conn, "model_usage", "reasoning_tokens"),
    );
    let (cw, cr) = (
        num_col(conn, "model_usage", "cache_creation_input_tokens"),
        num_col(conn, "model_usage", "cache_read_input_tokens"),
    );
    // umid 可空列：老版本缺列时整列降级 NULL（MAX 聚合对 NULL 行透明，
    // 组内同轮取值一致；全 NULL 组输出 NULL）
    let umid = opt_col(conn, "model_usage", "parent_user_message_id");
    let umid_expr = if umid == "NULL" {
        "NULL".to_string()
    } else {
        format!("MAX({umid})")
    };
    // psess 仅子代理会话有值：session 表/parent_id 缺失（老版本）时整列
    // 降级 NULL（放弃并入父会话，该 run 仅按 sess 命中子会话自身）
    let has_parent = has_table(conn, "session")
        && crate::db::has_column(conn, "session", "parent_id");
    let join = if has_parent {
        "LEFT JOIN session s ON s.id = mu.session_id"
    } else {
        ""
    };
    let psess_expr = if has_parent { "s.parent_id" } else { "NULL" };
    // 外层扫查限 7 天窗口（行数几万级可控），IN 子查询限定"近 10 分钟有
    // 请求"的轮——组内聚合含窗口外的早期请求（长轮完整合计）
    let sql = format!(
        "SELECT mu.turn_id, {umid_expr}, mu.session_id, {psess_expr}, \
         SUM({inp}), SUM({out}), SUM({cr}), SUM({cw}), SUM({rt}), COUNT(*), \
         MIN(mu.started_at) \
         FROM model_usage mu {join} \
         WHERE mu.started_at >= ?2 AND mu.turn_id IN \
           (SELECT turn_id FROM model_usage WHERE started_at >= ?1) \
         GROUP BY mu.turn_id, mu.session_id"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备进行中轮查询失败: {e}"))?;
    let rows = stmt
        .query_map(rusqlite::params![recent_start_ms, sweep_start_ms], |row| {
            Ok((
                row.get::<_, String>(0)?,
                UsageRun {
                    user_message_id: row.get(1)?,
                    session_id: row.get::<_, Option<String>>(2)?.unwrap_or_default(),
                    parent_session_id: row.get(3)?,
                    input_tokens: row.get::<_, Option<i64>>(4)?.unwrap_or(0).max(0),
                    output_tokens: row.get::<_, Option<i64>>(5)?.unwrap_or(0).max(0),
                    cache_read: row.get::<_, Option<i64>>(6)?.unwrap_or(0).max(0),
                    cache_write: row.get::<_, Option<i64>>(7)?.unwrap_or(0).max(0),
                    reasoning: row.get::<_, Option<i64>>(8)?.unwrap_or(0).max(0),
                    requests: row.get::<_, i64>(9)?.max(0),
                    start: row.get::<_, Option<i64>>(10)?.unwrap_or(0),
                    merged: None,
                    sub: None,
                },
            ))
        })
        .map_err(|e| format!("读取进行中轮失败: {e}"))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|e| format!("读取进行中轮失败: {e}"))?;
    let mut runs = Vec::new();
    for (turn_id, mut run) in rows {
        // 完成轮不走此通道（turn_usage 已有行的 turn_id 直接丢弃）
        if done_turn_ids.contains(&turn_id) {
            continue;
        }
        // 脏行防御：无会话 id / 无有效开始时刻的行不导出（与 turns 的
        // start > 0 口径一致）
        if run.session_id.is_empty() || run.start <= 0 {
            continue;
        }
        // psess 仅子代理会话携带（主会话 parent_id 理论为 NULL，此处按
        // 前缀强制归 None，与 turns 导出的 sess_subagent% 口径一致）
        if !run.session_id.starts_with("sess_subagent") {
            run.parent_session_id = None;
        }
        runs.push(run);
    }
    merge_sub_runs(&mut runs, sub_orphans);
    Ok(runs)
}

/// runs 侧子代理并入（V9）：把子代理实时消耗并进主会话行 sub，并给
/// 已并入的子代理行打 m:1 防双计标记。
/// - sub 聚合 = 同批内 psess 指向该主会话的子代理行（并行多子代理全并）
///   + 游离子代理完成轮（按 parent_session_id 归并，collect_turns/merge
///   阶段已防与 turns 侧整轮并入重复）；
/// - m:1 仅在父会话存在进行中主轮行时置位（其数值确已并入该行 sub）；
///   父会话暂无主轮行的子代理行不打标，由渲染端按 psess 直接并入会话
///   累计，主轮行出现后自动切换口径（sub 与 m 同批一致，无缝衔接）。
fn merge_sub_runs(runs: &mut [UsageRun], sub_orphans: &[SubTurnRow]) {
    // psess → 子代理实时聚合（子代理 runs 行与游离子代理完成轮同池）
    let mut agg: BTreeMap<String, SubAgg> = BTreeMap::new();
    for r in runs.iter() {
        if !r.session_id.starts_with("sess_subagent") {
            continue;
        }
        let Some(p) = r.parent_session_id.as_deref() else {
            continue;
        };
        let e = agg.entry(p.to_string()).or_insert_with(SubAgg::empty);
        e.add(
            r.requests,
            r.input_tokens,
            r.output_tokens,
            r.cache_read,
            r.cache_write,
            r.reasoning,
        );
    }
    for o in sub_orphans {
        let Some(p) = o.parent_session_id.as_deref() else {
            continue;
        };
        let e = agg.entry(p.to_string()).or_insert_with(SubAgg::empty);
        e.add(
            o.requests,
            o.input_tokens,
            o.output_tokens,
            o.cache_read,
            o.cache_write,
            o.reasoning,
        );
    }
    // 主会话行消费聚合；子代理行按"父会话存在主会话行"打 m:1
    let main_sessions: BTreeSet<String> = runs
        .iter()
        .filter(|r| !r.session_id.starts_with("sess_subagent"))
        .map(|r| r.session_id.clone())
        .collect();
    for r in runs.iter_mut() {
        if r.session_id.starts_with("sess_subagent") {
            if r
                .parent_session_id
                .as_deref()
                .is_some_and(|p| main_sessions.contains(p))
            {
                r.merged = Some(1);
            }
        } else if let Some(s) = agg.get(r.session_id.as_str()) {
            r.sub = Some(s.clone());
        }
    }
}

/// 子代理轮并入：按"父会话 + 时间覆盖"（主轮 start ≤ 子轮 start 且
/// 子轮 end ≤ 主轮 end，见模块头）匹配主轮后累加 token 与次数；
/// dur/ttft 保持主轮自身口径不变。返回 (并入明细对, 游离子代理完成轮)：
/// 未并入且"所属主轮尚未落库"的子轮不再直接丢弃，而是输出给 runs 侧
/// 聚合进主会话行 sub（V9，主轮进行期间实时反映子代理消耗）。防双计
/// 判定沿用本函数的时间窗口匹配逻辑：父会话存在"start ≤ 子轮 start <
/// end"的完成主轮 = 子轮开始时所属主轮已在运行且已落库（子轮越界完成
/// 等窗口不匹配边界），维持丢弃不输出；仅当父会话没有任何覆盖子轮开始
/// 时刻的完成主轮（即所属主轮未落库、不在 turns）时才作为游离子代理
/// 完成轮输出。
fn merge_subagent_turns(
    turns: &mut [UsageTurn],
    subs: Vec<SubTurnRow>,
) -> (Vec<(String, String)>, Vec<SubTurnRow>) {
    let mut merged: Vec<(String, String)> = Vec::new();
    let mut orphans: Vec<SubTurnRow> = Vec::new();
    for sub in subs {
        // 无完成时刻或无父会话的子轮无法做时间窗口匹配，直接丢弃
        let (Some(sub_end), Some(parent)) =
            (sub.completed_at, sub.parent_session_id.clone())
        else {
            continue;
        };
        let Some(t) = turns.iter_mut().find(|t| {
            t.session_id == parent && t.start <= sub.started_at && t.end.is_some_and(|e| sub_end <= e)
        }) else {
            // 未命中：仅"所属主轮未落库（不在 turns）"时输出给 runs 侧
            let owner_done = turns.iter().any(|t| {
                t.session_id == parent
                    && t.start <= sub.started_at
                    && t.end.is_some_and(|e| sub.started_at < e)
            });
            if !owner_done {
                orphans.push(sub);
            }
            continue;
        };
        t.input_tokens += sub.input_tokens;
        t.output_tokens += sub.output_tokens;
        t.cache_read += sub.cache_read;
        t.cache_write += sub.cache_write;
        t.reasoning += sub.reasoning;
        t.requests += sub.requests;
        t.retries += sub.retries;
        t.tool_calls += sub.tool_calls;
        let s = t.sub.get_or_insert_with(SubAgg::empty);
        s.add(
            sub.requests,
            sub.input_tokens,
            sub.output_tokens,
            sub.cache_read,
            sub.cache_write,
            sub.reasoning,
        );
        merged.push((t.turn_id.clone(), sub.turn_id));
    }
    (merged, orphans)
}

/// turn_id → 去重模型清单（保持首次出现顺序）
fn build_models_map(rows: Vec<(String, String)>) -> BTreeMap<String, Vec<String>> {
    let mut map: BTreeMap<String, Vec<String>> = BTreeMap::new();
    for (turn_id, model) in rows {
        let list = map.entry(turn_id).or_default();
        if !list.contains(&model) {
            list.push(model);
        }
    }
    map
}

/// 把模型清单拼进各主轮：主轮自身模型 + 并入子轮模型，去重后逗号拼接。
fn attach_models(
    turns: &mut [UsageTurn],
    models: &BTreeMap<String, Vec<String>>,
    merged_pairs: &[(String, String)],
) {
    for t in turns.iter_mut() {
        let mut list = models.get(&t.turn_id).cloned().unwrap_or_default();
        for (main_id, sub_id) in merged_pairs {
            if main_id == &t.turn_id {
                if let Some(sub_models) = models.get(sub_id) {
                    for m in sub_models {
                        if !list.contains(m) {
                            list.push(m.clone());
                        }
                    }
                }
            }
        }
        t.models = list.join(",");
    }
}

// ============================================================
// 序列化与原子写出
// ============================================================

/// 渲染 usage-data.js 完整内容。v 为数据契约版本（v2 起含 umid 字段，
/// 渲染端对 v !== 2 视为无效数据走静默路径）；runs 为进行中轮附加字段
/// （v2 格式不变，旧渲染脚本忽略未知字段平滑兼容，空数组也输出）；
/// ts 为"本次实际写出的时刻"（内容无变化跳写时文件保持上次的 ts，语义
/// 即最后一次数据变化的时刻，渲染端据此跳过无变化的重建与重渲染）。
fn render_usage_js(ts_ms: i64, turns_json: &str, runs_json: &str) -> String {
    format!(
        "window.__ZBAR_USAGE__ = {{\"v\":2,\"ts\":{ts_ms},\"turns\":{turns_json},\"runs\":{runs_json}}};\n"
    )
}

/// 变化检测 + 原子写：turns 与 runs 的序列化字节拼接后与上轮相同则跳过
/// 写盘（runs 聚合值参与序列化，实时跳动数据天然触发重写；ts 字段不参与
/// 比较——若参与则每轮 ts 都不同，跳写失效，ZCode 渲染层每 2 秒白重载
/// 一次文件）；需要写出时先写 .tmp 再 rename，Electron 侧不会读到半截
/// 文件。返回是否实际写盘。
fn write_if_changed(
    dir: &Path,
    cache: &mut Option<String>,
    turns_json: &str,
    runs_json: &str,
    ts_ms: i64,
) -> Result<bool, String> {
    let mut payload =
        String::with_capacity(turns_json.len() + runs_json.len() + 1);
    payload.push_str(turns_json);
    payload.push('\u{1}'); /* 不可见分隔符：防两段拼接的边界歧义 */
    payload.push_str(runs_json);
    if cache.as_deref() == Some(payload.as_str()) {
        return Ok(false);
    }
    let target = dir.join(store::USAGE_DATA_FILE);
    let tmp = dir.join(format!("{}.tmp", store::USAGE_DATA_FILE));
    fs::write(&tmp, render_usage_js(ts_ms, turns_json, runs_json))
        .map_err(|e| format!("写入 {} 失败: {e}", tmp.display()))?;
    fs::rename(&tmp, &target).map_err(|e| format!("替换 {} 失败: {e}", target.display()))?;
    *cache = Some(payload);
    Ok(true)
}

// ============================================================
// 单元测试（内存/临时 sqlite 构造，不依赖真实 ~/.zcode）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 构造一轮主会话轮（其余字段取典型值，测试按需覆写）
    fn turn(id: &str, sess: &str, start: i64, end: Option<i64>) -> UsageTurn {
        UsageTurn {
            turn_id: id.to_string(),
            user_message_id: None,
            session_id: sess.to_string(),
            status: "completed".to_string(),
            start,
            end,
            input_tokens: 100,
            output_tokens: 200,
            cache_read: 50,
            cache_write: 10,
            reasoning: 5,
            requests: 2,
            retries: 0,
            tool_calls: 1,
            dur: Some(4000),
            ttft: Some(900),
            sub: None,
            models: String::new(),
        }
    }

    /// 构造一条子代理轮原始行（其余字段取典型值，测试按需覆写）
    fn sub_turn(
        id: &str,
        _sess: &str,
        parent: &str,
        start: i64,
        end: Option<i64>,
    ) -> SubTurnRow {
        SubTurnRow {
            turn_id: id.to_string(),
            parent_session_id: Some(parent.to_string()),
            started_at: start,
            completed_at: end,
            input_tokens: 30,
            output_tokens: 40,
            cache_read: 20,
            cache_write: 0,
            reasoning: 0,
            requests: 1,
            retries: 0,
            tool_calls: 0,
        }
    }

    #[test]
    fn 序列化_键名契约与渲染形态() {
        let mut t = turn("turn_a", "sess_1", 1000, Some(5000));
        t.status = "cancelled".to_string();
        t.user_message_id = Some("msg_u1".to_string());
        t.sub = Some(SubAgg {
            n: 2,
            requests: 5,
            input_tokens: 60,
            output_tokens: 80,
            cache_read: 40,
            cache_write: 0,
            reasoning: 0,
        });
        t.models = "GLM-5.3".to_string();
        let turns = vec![t];
        let json = serde_json::to_string(&turns).unwrap();
        // 短键名契约一字不差（usage.js 按名消费）
        for key in [
            "\"turn\":\"turn_a\"",
            "\"umid\":\"msg_u1\"",
            "\"sess\":\"sess_1\"",
            "\"status\":\"cancelled\"",
            "\"start\":1000",
            "\"end\":5000",
            "\"in\":100",
            "\"out\":200",
            "\"cr\":50",
            "\"cw\":10",
            "\"rt\":5",
            "\"req\":2",
            "\"retry\":0",
            "\"tool\":1",
            "\"dur\":4000",
            "\"ttft\":900",
            "\"models\":\"GLM-5.3\"",
        ] {
            assert!(json.contains(key), "缺少契约键 {key}：{json}");
        }
        // 子聚合对象
        assert!(
            json.contains("\"sub\":{\"n\":2,\"req\":5,\"in\":60,\"out\":80,\"cr\":40,\"cw\":0,\"rt\":0}"),
            "sub 聚合形态不符：{json}"
        );
        // 完整文件形态：v/ts/turns/runs 四字段 + 分号结尾（v2 格式不变，
        // runs 为 V6 起追加的进行中轮字段；runs 为空时也输出）
        let file = render_usage_js(12345, &json, "[]");
        assert!(
            file.starts_with(
                "window.__ZBAR_USAGE__ = {\"v\":2,\"ts\":12345,\"turns\":"
            ),
            "{file}"
        );
        assert!(file.contains(",\"runs\":[]};\n"), "{file}");
        assert!(file.ends_with("};\n"));
    }

    #[test]
    fn runs序列化_键名契约与短键形态() {
        let r = UsageRun {
            user_message_id: Some("msg_u1".to_string()),
            session_id: "sess_1".to_string(),
            parent_session_id: None,
            input_tokens: 700,
            output_tokens: 80,
            cache_read: 600,
            cache_write: 20,
            reasoning: 9,
            requests: 3,
            start: 42,
            merged: None,
            sub: None,
        };
        let json = serde_json::to_string(&vec![r]).unwrap();
        // 短键名契约一字不差（usage.js 按名消费）；无 turn/status 等整轮字段；
        // m/sub 为 None 时不输出（V9 附加字段，主会话行无子代理并入时保持
        // 旧形态，向后兼容）
        assert_eq!(
            json,
            "[{\"umid\":\"msg_u1\",\"sess\":\"sess_1\",\"psess\":null,\
              \"in\":700,\"out\":80,\"cr\":600,\"cw\":20,\"rt\":9,\
              \"req\":3,\"start\":42}]",
            "runs 行序列化形态不符：{json}"
        );
        // umid null + 子代理 psess 形态
        let sub_run = UsageRun {
            user_message_id: None,
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 0,
            start: 1,
            merged: None,
            sub: None,
        };
        let json = serde_json::to_string(&vec![sub_run]).unwrap();
        assert!(json.contains("\"umid\":null"), "{json}");
        assert!(json.contains("\"psess\":\"sess_main\""), "{json}");
        assert!(!json.contains("\"m\":"), "未并入的子代理行不打 m 标记：{json}");
        // V9：m:1 标记 + sub 聚合的短键形态（结构与 turns 行 sub 一致）
        let merged_run = UsageRun {
            user_message_id: Some("msg_main".to_string()),
            session_id: "sess_main".to_string(),
            parent_session_id: None,
            input_tokens: 100,
            output_tokens: 200,
            cache_read: 50,
            cache_write: 10,
            reasoning: 5,
            requests: 2,
            start: 7,
            merged: None,
            sub: Some(SubAgg {
                n: 3,
                requests: 9,
                input_tokens: 90,
                output_tokens: 120,
                cache_read: 60,
                cache_write: 0,
                reasoning: 1,
            }),
        };
        let json = serde_json::to_string(&vec![merged_run]).unwrap();
        assert!(
            json.contains("\"sub\":{\"n\":3,\"req\":9,\"in\":90,\"out\":120,\"cr\":60,\"cw\":0,\"rt\":1}"),
            "sub 聚合形态应与 turns 行 sub 一致：{json}"
        );
        assert!(!json.contains("\"m\":"), "主会话行不打 m 标记：{json}");
        // 子代理行 m:1
        let marked = UsageRun {
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            merged: Some(1),
            ..sub_run_template()
        };
        let json = serde_json::to_string(&vec![marked]).unwrap();
        assert!(json.contains(",\"m\":1}"), "已并入主轮行 sub 的子代理行应带 m:1：{json}");
        assert!(!json.contains("\"sub\":"), "子代理行不携带 sub：{json}");
    }

    /// 序列化测试用子代理行模板（测试按需覆写）
    fn sub_run_template() -> UsageRun {
        UsageRun {
            user_message_id: None,
            session_id: "sess_subagent_agent_1".to_string(),
            parent_session_id: Some("sess_main".to_string()),
            input_tokens: 0,
            output_tokens: 0,
            cache_read: 0,
            cache_write: 0,
            reasoning: 0,
            requests: 0,
            start: 1,
            merged: None,
            sub: None,
        }
    }

    #[test]
    fn 写盘_内容不变跳写_变化才重写且无临时残留() {
        let dir = std::env::temp_dir().join(format!(
            "zbar-usage-feed-write-{}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let target = dir.join(store::USAGE_DATA_FILE);

        let mut cache: Option<String> = None;
        // 首次：写盘
        assert!(
            write_if_changed(&dir, &mut cache, "[{\"turn\":\"a\"}]", "[]", 1000).unwrap()
        );
        let first = fs::read_to_string(&target).unwrap();
        assert!(first.contains("\"ts\":1000"), "{first}");
        assert!(first.contains("\"runs\":[]"), "{first}");
        // 内容无变化（仅 ts 不同）→ 跳写，文件保持旧 ts
        assert!(
            !write_if_changed(&dir, &mut cache, "[{\"turn\":\"a\"}]", "[]", 2000).unwrap()
        );
        assert_eq!(
            fs::read_to_string(&target).unwrap(),
            first,
            "内容未变时不应重写文件"
        );
        // turns 内容变化 → 重写为新 ts
        assert!(
            write_if_changed(&dir, &mut cache, "[{\"turn\":\"b\"}]", "[]", 3000).unwrap()
        );
        let third = fs::read_to_string(&target).unwrap();
        assert!(third.contains("\"ts\":3000"), "{third}");
        assert!(third.contains("\"turn\":\"b\""));
        // runs 内容变化（turns 不变）同样触发重写——进行中轮实时聚合
        // 每 2 秒跳动天然走到这里
        assert!(
            write_if_changed(
                &dir,
                &mut cache,
                "[{\"turn\":\"b\"}]",
                "[{\"sess\":\"s1\"}]",
                4000
            )
            .unwrap()
        );
        let fourth = fs::read_to_string(&target).unwrap();
        assert!(fourth.contains("\"ts\":4000"), "{fourth}");
        assert!(fourth.contains("\"runs\":[{\"sess\":\"s1\"}]"), "{fourth}");
        // 原子写不留 .tmp 残留
        assert!(!dir.join(format!("{}.tmp", store::USAGE_DATA_FILE)).exists());

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 子代理并入_时间窗口聚合_未命中分流游离子轮与越界丢弃() {
        let mut turns = vec![
            turn("turn_main", "sess_main", 1000, Some(5000)),
            turn("turn_other", "sess_other", 1000, Some(9000)),
        ];
        let subs = vec![
            // 命中：父会话一致且时间被主轮覆盖
            sub_turn("turn_sub1", "sess_sub_a", "sess_main", 1100, Some(1500)),
            sub_turn("turn_sub2", "sess_sub_b", "sess_main", 2000, Some(3000)),
            // 未命中一：父会话不一致（属于别的会话）
            sub_turn("turn_sub3", "sess_sub_c", "sess_other", 1200, Some(1400)),
            // 未命中二：完成时刻缺失
            SubTurnRow {
                completed_at: None,
                ..sub_turn("turn_sub4", "sess_sub_d", "sess_main", 1100, Some(1500))
            },
            // 未命中三：时间不被任何主轮覆盖（早于主轮开始）→ 所属主轮
            // 尚未落库，作为游离子代理完成轮输出给 runs 侧聚合
            sub_turn("turn_sub5", "sess_sub_e", "sess_main", 500, Some(900)),
            // 未命中四：子轮开始于主轮窗口内但完成晚于主轮完成（越界）→
            // 所属主轮已落库（覆盖子轮开始时刻），维持丢弃防双计
            sub_turn("turn_sub6", "sess_sub_f", "sess_main", 2000, Some(6000)),
        ];
        let (pairs, orphans) = merge_subagent_turns(&mut turns, subs);

        // 并入明细：仅三条命中（turn_main ← sub1/sub2；turn_other ← sub3）
        assert_eq!(
            pairs,
            vec![
                ("turn_main".to_string(), "turn_sub1".to_string()),
                ("turn_main".to_string(), "turn_sub2".to_string()),
                ("turn_other".to_string(), "turn_sub3".to_string()),
            ]
        );

        let main = turns.iter().find(|t| t.turn_id == "turn_main").unwrap();
        // token 与次数累加（自身 100/200/50 + 两条子轮 30+30 / 40+40 / 20+20）
        assert_eq!(main.input_tokens, 160);
        assert_eq!(main.output_tokens, 280);
        assert_eq!(main.cache_read, 90);
        assert_eq!(main.requests, 4);
        // dur/ttft 保持主轮自身口径，不随并入变化
        assert_eq!(main.dur, Some(4000));
        assert_eq!(main.ttft, Some(900));
        // 子聚合明细（次数与 token 合计）
        let sub = main.sub.as_ref().expect("并入后应有子聚合");
        assert_eq!(sub.n, 2);
        assert_eq!(sub.requests, 2);
        assert_eq!(sub.input_tokens, 60);
        assert_eq!(sub.output_tokens, 80);
        assert_eq!(sub.cache_read, 40);

        let other = turns.iter().find(|t| t.turn_id == "turn_other").unwrap();
        assert_eq!(other.input_tokens, 130, "另一会话主轮应并入自己的子轮");
        assert_eq!(other.sub.as_ref().unwrap().n, 1);

        // 游离子代理完成轮：仅 sub5（所属主轮未落库）；sub4（无完成时刻）
        // 与 sub6（越界，所属主轮已落库）不输出
        assert_eq!(orphans.len(), 1, "游离子轮应仅含 sub5：{orphans:?}");
        assert_eq!(orphans[0].turn_id, "turn_sub5");
    }

    #[test]
    fn 模型清单_主轮并入子轮去重拼接() {
        let mut turns = vec![turn("turn_main", "sess_main", 1000, Some(5000))];
        let mut models = BTreeMap::new();
        models.insert("turn_main".to_string(), vec!["GLM-5.3".to_string()]);
        models.insert(
            "turn_sub1".to_string(),
            vec!["GLM-5.3".to_string(), "GLM-4.7".to_string()],
        );
        let pairs = vec![("turn_main".to_string(), "turn_sub1".to_string())];
        attach_models(&mut turns, &models, &pairs);
        assert_eq!(turns[0].models, "GLM-5.3,GLM-4.7", "应去重合并主轮与子轮模型");

        // 无模型记录的轮 → 空串（前端不显示模型）
        let mut turns = vec![turn("turn_x", "sess", 1, Some(2))];
        attach_models(&mut turns, &BTreeMap::new(), &[]);
        assert_eq!(turns[0].models, "");
    }

    #[test]
    fn build_models_map_去重保序() {
        let map = build_models_map(vec![
            ("t1".into(), "b-model".into()),
            ("t1".into(), "a-model".into()),
            ("t1".into(), "b-model".into()), // 重复
            ("t2".into(), "m".into()),
        ]);
        assert_eq!(map.get("t1").unwrap(), &vec!["b-model".to_string(), "a-model".to_string()]);
        assert_eq!(map.get("t2").unwrap(), &vec!["m".to_string()]);
    }

    /// 临时 sqlite 库（文件形， rusqlite 直连，测试结束清理）
    fn temp_db(name: &str) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-usage-feed-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        (conn, path)
    }

    #[test]
    fn 无turn_usage表时功能禁用_核心列缺失同样禁用() {
        // 场景一：只有 session 表，无 turn_usage 表 → Ok(None) 功能关闭
        let (conn, path) = temp_db("no-table");
        conn.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);")
            .unwrap();
        let out = collect_turns(&conn, 0).unwrap();
        assert!(out.is_none(), "无 turn_usage 表应返回 None 禁用功能");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：turn_usage 表存在但缺核心列（极端老版本）→ Ok(None)
        let (conn, path) = temp_db("missing-core");
        conn.execute_batch("CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT);")
            .unwrap();
        let out = collect_turns(&conn, 0).unwrap();
        assert!(out.is_none(), "缺核心列应返回 None 禁用功能");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 有表时降级查询_缺列按0与null兜底() {
        // 老版本形态：turn_usage 只有 4 个核心列（无任何统计/耗时列），
        // 也无 session / model_usage 表 → 能出数据、数值全 0、耗时为 null
        let (conn, path) = temp_db("minimal");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000)",
            [],
        )
        .unwrap();
        let (out, orphans) = collect_turns(&conn, 0).unwrap().expect("核心列齐备应有输出");
        assert_eq!(out.len(), 1);
        let t = &out[0];
        assert_eq!(t.turn_id, "turn_1");
        assert_eq!(t.status, "completed");
        assert_eq!(t.input_tokens, 0, "缺列应按 0 降级");
        assert_eq!(t.output_tokens, 0);
        assert_eq!(t.dur, None, "缺列应按 null 降级");
        assert_eq!(t.ttft, None);
        assert_eq!(t.sub, None, "无 session 表时放弃并入");
        assert_eq!(t.models, "", "无 model_usage 表时无模型清单");
        assert!(orphans.is_empty(), "无子轮时无游离子代理完成轮");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 有表时正常读出_主会话与子代理拆分() {
        // 完整 schema（与 v3.10.1 实测一致的关键列）+ 主/子代理行混合
        let (conn, path) = temp_db("full");
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, duration_ms INTEGER,
                time_to_first_token_ms INTEGER, model_request_count INTEGER,
                model_retry_count INTEGER, tool_call_count INTEGER,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);
             CREATE TABLE model_usage (turn_id TEXT, model_id TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_m', 'completed', 1000, 5000, 4000, 900, 2, 0, 1, 100, 200, 5, 10, 50),
               ('sess_subagent_agent_1', 'turn_s', 'completed', 1100, 1500, 350, 100, 1, 0, 0, 30, 40, 0, 0, 20);
             INSERT INTO model_usage VALUES
               ('turn_m', 'GLM-5.3', 1000), ('turn_s', 'GLM-4.7', 1100);",
        )
        .unwrap();
        let (out, orphans) = collect_turns(&conn, 0).unwrap().expect("应能读出");
        // 子代理轮不单独出现在导出序列
        assert_eq!(out.len(), 1, "子代理轮不应单独导出：{out:?}");
        let t = &out[0];
        assert_eq!(t.turn_id, "turn_m");
        // 子代理已并入（30/40/20 + 自身 100/200/50）
        assert_eq!(t.input_tokens, 130);
        assert_eq!(t.output_tokens, 240);
        assert_eq!(t.cache_read, 70);
        assert_eq!(t.requests, 3);
        let sub = t.sub.as_ref().expect("应有子聚合");
        assert_eq!(sub.n, 1);
        // 模型清单：主轮 + 并入子轮去重
        assert_eq!(t.models, "GLM-5.3,GLM-4.7");
        // 子轮已整轮并入 turns → 不应再分流到 runs 侧（防双计）
        assert!(orphans.is_empty(), "已并入 turns 的子轮不应进游离集合：{orphans:?}");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 导出窗口与上限常量_7天3000轮() {
        assert_eq!(WINDOW_MS, 7 * 24 * 3600 * 1000, "导出窗口应为 7 天");
        assert_eq!(MAX_TURNS, 3000, "导出行数上限应为 3000 轮");
        assert_eq!(RUN_WINDOW_MS, 10 * 60 * 1000, "runs 新鲜度窗口应为 10 分钟");
    }

    #[test]
    fn umid导出_列存在取值_缺失或null输出null() {
        // 场景一：user_message_id 列存在且有值 / 值为 null
        let (conn, path) = temp_db("umid-full");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                user_message_id TEXT);",
        )
        .unwrap();
        conn.execute_batch(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000, 'msg_abc'),
             ('sess_1', 'turn_2', 'completed', 2000, NULL);",
        )
        .unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), 2);
        assert_eq!(
            out[0].user_message_id.as_deref(),
            Some("msg_abc"),
            "有值应原样导出（DOM 匹配键）"
        );
        assert_eq!(out[1].user_message_id, None, "值为 null 应导出 null");
        // 序列化形态：None → "umid":null（消费端按 null 跳过匹配）
        let json = serde_json::to_string(&out).unwrap();
        assert!(json.contains("\"umid\":\"msg_abc\""), "{json}");
        assert!(json.contains("\"umid\":null"), "{json}");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：user_message_id 列缺失（老版本库）→ 整列降级 null
        let (conn, path) = temp_db("umid-missing-col");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        conn.execute(
            "INSERT INTO turn_usage VALUES ('sess_1', 'turn_1', 'completed', 1000)",
            [],
        )
        .unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].user_message_id, None, "缺列应降级导出 null");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 导出上限_超出3000轮保留最新() {
        let (conn, path) = temp_db("cap");
        conn.execute_batch(
            "CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);",
        )
        .unwrap();
        // 插入 3005 轮（started_at 递增），应仅保留最新 3000 轮
        let mut sql = String::from(
            "INSERT INTO turn_usage (session_id, turn_id, status, started_at) VALUES",
        );
        for i in 0..(MAX_TURNS + 5) {
            if i > 0 {
                sql.push(',');
            }
            sql.push_str(&format!("('sess_1', 'turn_{i}', 'completed', {})", 1000 + i));
        }
        conn.execute_batch(&sql).unwrap();
        let (out, _orphans) = collect_turns(&conn, 0).unwrap().expect("应有输出");
        assert_eq!(out.len(), MAX_TURNS, "应截断到 3000 轮");
        // 序列按 started_at 升序：保留的是最新的 3000 轮（丢弃头部 5 条最旧的）
        assert_eq!(out[0].turn_id, "turn_5", "最旧的 5 轮应被丢弃");
        assert_eq!(
            out.last().unwrap().turn_id,
            format!("turn_{}", MAX_TURNS + 4),
            "最新一轮应保留在末尾"
        );
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// runs 测试用完整库（session + turn_usage + model_usage）
    fn runs_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                user_message_id TEXT);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                parent_user_message_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn runs聚合_多请求求和与计数_完成轮与陈旧轮排除() {
        let (conn, path) = runs_db("agg");
        // 固定"当前时刻"：now、近 10 分钟窗口起点、7 天扫查窗口起点
        let now = 1_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_main', NULL);
             -- 进行中轮：3 行在近 10 分钟内 + 1 行在 20 分钟前（长轮早期
             -- 请求，不在新鲜度窗口但在扫查窗口内，聚合应完整计入）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, output_tokens,
                reasoning_tokens, cache_creation_input_tokens,
                cache_read_input_tokens) VALUES
               ('sess_main', 'turn_live', {m1}, 'msg_live', 100, 200, 5, 10, 50),
               ('sess_main', 'turn_live', {m2}, 'msg_live', 110, 21, 0, 0, 60),
               ('sess_main', 'turn_live', {m3}, 'msg_live', 120, 22, 7, 2, 70),
               ('sess_main', 'turn_live', {m4}, 'msg_live', 130, 23, 0, 0, 80);
             -- 已完成轮：model_usage 有行但 turn_usage 也有行 → 排除
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_main', 'turn_done', {d1}, 'msg_done', 999);
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_done', 'completed', {d2}, 'msg_done');
             -- 陈旧轮：请求全部早于新鲜度窗口 → 排除
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_main', 'turn_old', {o1}, 'msg_old', 888);",
            m1 = now - 1_000,
            m2 = now - 2_000,
            m3 = now - 3_000,
            m4 = now - 1_200_000, /* 20 分钟前：新鲜度窗口外 */
            d1 = now - 5_000,
            d2 = now - 4_000,
            o1 = now - 1_200_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        assert!(done.contains("turn_done"), "完成轮应进入 done 集合");
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1, "仅 turn_live 应出现在 runs：{runs:?}");
        let r = &runs[0];
        assert_eq!(r.session_id, "sess_main");
        assert_eq!(r.user_message_id.as_deref(), Some("msg_live"));
        assert_eq!(r.parent_session_id, None, "主会话 psess 应为 null");
        // 4 行完整聚合（含新鲜度窗口外的长轮早期请求）
        assert_eq!(r.input_tokens, 100 + 110 + 120 + 130);
        assert_eq!(r.output_tokens, 200 + 21 + 22 + 23);
        assert_eq!(r.cache_read, 50 + 60 + 70 + 80);
        assert_eq!(r.cache_write, 10 + 2);
        assert_eq!(r.reasoning, 5 + 7);
        assert_eq!(r.requests, 4, "req 应为 model_usage 行数");
        assert_eq!(r.start, now - 1_200_000, "start 应为最早请求时刻");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_子代理psess并入父会话_窗口过滤() {
        let (conn, path) = runs_db("psess");
        let now = 2_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               -- 子代理进行中轮：umid 指向子代理自己的消息（主会话 DOM
               -- 匹配不到），psess 指回父会话
               ('sess_subagent_agent_1', 'turn_s', {t1}, 'msg_child', 30),
               -- 主会话进行中轮
               ('sess_main', 'turn_m', {t2}, 'msg_main', 100);",
            t1 = now - 4_000,
            t2 = now - 2_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 2, "{runs:?}");
        let sub = runs.iter().find(|r| r.session_id == "sess_subagent_agent_1");
        let main = runs.iter().find(|r| r.session_id == "sess_main");
        assert_eq!(
            sub.map(|r| r.parent_session_id.as_deref()),
            Some(Some("sess_main")),
            "子代理 run 的 psess 应指回父会话"
        );
        assert_eq!(
            sub.map(|r| r.user_message_id.as_deref()),
            Some(Some("msg_child"))
        );
        assert_eq!(
            main.map(|r| r.parent_session_id.as_deref()),
            Some(None),
            "主会话 run 的 psess 应为 null"
        );
        // V9：父会话存在进行中主轮行 → 子代理行打 m:1（其值已并入主轮行
        // sub）；主会话行不带 m、携带 sub 聚合（n=1，子代理行 30 in）
        assert_eq!(sub.map(|r| r.merged), Some(Some(1)), "子代理行应带 m:1：{runs:?}");
        assert_eq!(main.map(|r| r.merged), Some(None), "主会话行不打 m：{runs:?}");
        let main_sub = main.and_then(|r| r.sub.as_ref()).expect("主会话行应有 sub");
        assert_eq!(main_sub.n, 1);
        assert_eq!(main_sub.input_tokens, 30);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// V9 端到端库：完整 schema（含 completed_at / 统计列），供
    /// collect_turns → collect_runs 全管线断言
    fn v9_db(name: &str) -> (Connection, std::path::PathBuf) {
        let (conn, path) = temp_db(name);
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER,
                completed_at INTEGER, user_message_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                model_request_count INTEGER, model_retry_count INTEGER, tool_call_count INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                parent_user_message_id TEXT, model_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn runs_主轮sub聚合_并行子代理与游离子轮_m防双计() {
        let (conn, path) = v9_db("v9-agg");
        let now = 5_000_000_000_i64;
        let recent = now - RUN_WINDOW_MS;
        let sweep = now - WINDOW_MS;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_a', 'sess_main'),
               ('sess_subagent_b', 'sess_main'), ('sess_subagent_c', 'sess_main');
             -- 主会话进行中轮（首笔请求已完成，主轮 runs 行存在）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 100, 'GLM-5.3');
             -- 并行两个子代理进行中轮（多子代理并行的实时行）
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_a', 'turn_sa', {m2}, 'msg_sub_a', 30, 'GLM-4.7'),
               ('sess_subagent_a', 'turn_sa', {m2}, 'msg_sub_a', 5, 'GLM-4.7'),
               ('sess_subagent_b', 'turn_sb', {m3}, 'msg_sub_b', 40, 'GLM-4.7');
             -- 游离子代理完成轮：子代理 turn_usage 已落库，但所属主轮
             -- turn_m 尚未落库 → 不进 turns，分流给 runs 侧聚合
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens, model_id) VALUES
               ('sess_subagent_a', 'turn_s_orphan', {o1}, 'msg_sub_a', 50, 'GLM-4.7');
             INSERT INTO turn_usage VALUES
               ('sess_subagent_a', 'turn_s_orphan', 'completed', {o1}, {o2},
                'msg_sub_a', 50, 60, 0, 0, 20, 3, 0, 0);
             -- 已完成主轮及其子轮：整轮并入 turns（不得进 runs.sub 防双计）
             INSERT INTO turn_usage VALUES
               ('sess_main', 'turn_m_done', 'completed', {d1}, {d2},
                'msg_done', 100, 200, 0, 0, 50, 2, 0, 1),
               ('sess_subagent_c', 'turn_s_merged', 'completed', {d3}, {d4},
                'msg_sub_c', 70, 80, 0, 0, 30, 5, 0, 0);",
            m1 = now - 1_000,
            m2 = now - 2_000,
            m3 = now - 3_000,
            o1 = now - 4_000,
            o2 = now - 3_500,
            d1 = now - 60_000,
            d2 = now - 50_000,
            d3 = now - 58_000,
            d4 = now - 52_000,
        ))
        .unwrap();
        let (turns, orphans) = collect_turns(&conn, sweep).unwrap().expect("应有输出");
        // 完成侧：仅主会话完成轮，其子轮已整轮并入（sub.n=1, in=70）
        assert_eq!(turns.len(), 1, "{turns:?}");
        let done_turn = &turns[0];
        assert_eq!(done_turn.turn_id, "turn_m_done");
        let done_sub = done_turn.sub.as_ref().expect("完成轮应有子聚合");
        assert_eq!(done_sub.n, 1);
        assert_eq!(done_sub.input_tokens, 70);
        assert_eq!(done_sub.requests, 5);
        // 游离子代理完成轮：仅未落库主轮的 turn_s_orphan
        assert_eq!(orphans.len(), 1, "{orphans:?}");
        assert_eq!(orphans[0].turn_id, "turn_s_orphan");
        assert_eq!(orphans[0].parent_session_id.as_deref(), Some("sess_main"));
        assert_eq!(orphans[0].input_tokens, 50);
        assert_eq!(orphans[0].requests, 3);

        // runs 侧：主轮行 sub = 并行子代理行 + 游离子代理完成轮
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, recent, sweep, &done, &orphans).unwrap();
        assert_eq!(runs.len(), 3, "仅 3 个进行中轮：{runs:?}");
        let main = runs.iter().find(|r| r.session_id == "sess_main").unwrap();
        assert_eq!(main.merged, None, "主会话行不打 m");
        let sub = main.sub.as_ref().expect("主轮行应有 sub 聚合");
        // n = 2 条子代理实时行 + 1 条游离子代理完成轮
        assert_eq!(sub.n, 3);
        assert_eq!(sub.requests, 2 + 1 + 3);
        // 防双计：恰好 = 子代理实时行（30+5+40）+ 游离子轮（50），
        // 已并入 turns 的 turn_s_merged（in=70）不在其中
        assert_eq!(sub.input_tokens, 125);
        // 子代理实时行：父会话存在主轮行 → 打 m:1（渲染端会话累计跳过）
        for r in runs.iter().filter(|r| r.session_id != "sess_main") {
            assert_eq!(r.merged, Some(1), "子代理行应带 m:1：{r:?}");
            assert_eq!(r.sub, None, "子代理行不携带 sub");
        }
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_子代理行父会话无主行时不打m标记() {
        // 主轮首笔请求未完成（sess_main 无 model_usage 行）时子代理行不打
        // m:1——渲染端会话条按 psess 直接并入，主轮行出现后自动切换口径
        let (conn, path) = runs_db("v9-no-main");
        let now = 6_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_main', NULL), ('sess_subagent_agent_1', 'sess_main');
             INSERT INTO model_usage (session_id, turn_id, started_at,
                parent_user_message_id, input_tokens) VALUES
               ('sess_subagent_agent_1', 'turn_s', {t1}, 'msg_child', 30);",
            t1 = now - 2_000,
        ))
        .unwrap();
        let sweep = now - WINDOW_MS;
        let done = collect_done_turn_ids(&conn, sweep).unwrap();
        let runs = collect_runs(&conn, now - RUN_WINDOW_MS, sweep, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1, "{runs:?}");
        assert_eq!(runs[0].session_id, "sess_subagent_agent_1");
        assert_eq!(
            runs[0].merged, None,
            "父会话无主轮行时不打 m（会话条按 psess 兜底并入）"
        );
        assert_eq!(runs[0].sub, None);
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_无表或无行时输出空数组() {
        let now = 3_000_000_000_i64;
        // 场景一：model_usage 表缺失（老版本库）→ 空 runs，不报错
        let (conn, path) = temp_db("runs-no-mu");
        conn.execute_batch(
            "CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT, status TEXT,
                started_at INTEGER);",
        )
        .unwrap();
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert!(runs.is_empty(), "无 model_usage 表应为空 runs");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 场景二：表存在但窗口内无行 → 空 runs
        let (conn, path) = runs_db("runs-empty");
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert!(runs.is_empty(), "无行应为空 runs");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn runs_列缺失按0与null降级() {
        // 老版本形态：model_usage 只有 3 个核心列（无统计列、无 umid 列），
        // session 表也缺失 → 数值全 0、umid/psess 双 null、req 正确计数
        let (conn, path) = temp_db("runs-minimal");
        conn.execute_batch(
            "CREATE TABLE turn_usage (session_id TEXT, turn_id TEXT, status TEXT,
                started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER);",
        )
        .unwrap();
        let now = 4_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ('sess_1', 'turn_1', {t1}), ('sess_1', 'turn_1', {t2});",
            t1 = now - 1_000,
            t2 = now - 2_000,
        ))
        .unwrap();
        let done = collect_done_turn_ids(&conn, now - WINDOW_MS).unwrap();
        let runs =
            collect_runs(&conn, now - RUN_WINDOW_MS, now - WINDOW_MS, &done, &[]).unwrap();
        assert_eq!(runs.len(), 1);
        let r = &runs[0];
        assert_eq!(r.user_message_id, None, "缺 umid 列应降级 null");
        assert_eq!(r.parent_session_id, None, "缺 session 表应降级 null");
        assert_eq!(r.input_tokens, 0, "缺统计列应按 0 降级");
        assert_eq!(r.output_tokens, 0);
        assert_eq!(r.requests, 2, "req 计数不依赖统计列");
        assert_eq!(r.start, now - 2_000);
        assert_eq!(r.merged, None, "主会话行不打 m");
        assert_eq!(r.sub, None, "无子代理时无 sub 聚合");
        drop(conn);
        let _ = fs::remove_file(&path);
    }
}

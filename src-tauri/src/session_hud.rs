//! 会话 Token 悬浮窗（Session HUD）：独立透明置顶小窗，实时展示当前
//! 活跃 ZCode 会话的 token 消耗（多会话列表，每会话一行）。
//!
//! 数据源：ZCode 主库 ~/.zcode/cli/db/db.sqlite（只读连接，绝不写主库），
//! 不依赖 agent_theme 的 asar 注入体系。本模块完全照搬 pet.rs 的成熟
//! 骨架（配置持久化 / 动态建窗的平台差异处理 / run_on_main_thread 死锁
//! 防护 / Moved 节流落盘 / Destroyed 停轮询复位 / 独立轮询线程），仅把
//! "宠物状态摘要"换成"会话快照"。
//!
//! 活跃判定口径：会话在时间窗内有 model_usage 落库（每笔模型请求完成即
//! 写一行，实时性最好）或 message 落库（用户发消息即写，覆盖首笔请求未
//! 返回的空窗期，两信号取并集）。默认 10 分钟，档位 5/10/30/不限（不限
//! 仍有 24h 兜底上限，防列表无限长）。排序按最近活动时间倒序（正在生成
//! 的会话天然在最顶部），只显示前 MAX_VISIBLE_SESSIONS 条，超出折叠。
//!
//! 口径区分：活跃判定看时间窗内；展示主体是"会话树"（主会话 + 全部
//! 子代理，注入版 V9 合计口径）——子代理活动归并到主会话（last_active
//! 取 max，主会话自身空闲但子代理在跑时整行不消失），累计为树内
//! model_usage 合计且子代理永不单独成行（防双计，注入版 m:1 同思路）。
//! 字段口径对齐注入版 usage.js 会话条（renderSessionBar）并补 TTFT：
//! ↑ = Σ max(0, input − cache_read)（非缓存输入，注入版"保守取小值"）、
//! ↓ = Σ output、⟲ = Σ cache_read、Σ = ↑+↓+⟲（注入版 V15 口径）、
//! × = model_usage 行数（每行一笔请求）；模型集合为会话树时间窗内
//! distinct model_id（按最近使用降序，注入版 models 口径）。
//!
//! 动态速度（真动态）：DB 的请求粒度太粗，速度走 rollout 旁路——
//! ~/.zcode/cli/rollout/model-io-{sessionId}.jsonl 每笔请求完成即追加
//! 一行（含 usage.outputTokens），偏移续读增量解析出样本
//!（start=startedAt，end=读取时刻，rate=output/max(1s, end−start)），
//! 窗口速度 = 样本与最近 4s 窗口的重叠时长加权均值（1 秒节拍刷新，
//! DB 查询隔拍执行等效 2 秒不变）。生成中显示窗口速度（长请求进行中
//! 无新行时保持最后非零），空闲归 0.0；rollout 全缺失/全解析失败回退
//! DB 最近一笔完成请求口径（TTFT 恒取该值，静态参考），缺失 "–"。
//! 铁律：rollout 只服务速度展示，绝不并入 Σ/↑/↓/⟲/× 累计口径（累计
//! 以 db.sqlite model_usage 为唯一来源，防双计）。
//!
//! 生成中判定：会话树内任一成员最新一轮未完成（与 usage_feed 的 runs
//! 口径同源——turn_usage 尚无该 turn 的完成行）；成员最新 model_usage
//! 行本身为 error/cancelled 时不判生成中（失败轮可能永不落 turn 行，
//! 防状态点永久卡"生成中"）。子代理会话不进列表（已并入主会话），
//! 避免内部代理行淹没用户视角的会话。
//!
//! 与 pet.rs 一致的铁律：
//! - 同步 invoke 命令里创建/操作窗口必须经 app.run_on_main_thread +
//!   channel + 超时等待（Windows 上同步命令占主线程直接建窗会自等待
//!   死锁，pet.rs 有事故记载）；
//! - macOS 建窗后 set_background_color(透明) 防白底；Windows 走
//!   隐藏建窗 → set_size → show（规避 WM_GETMINMAXINFO 钳宽）；
//! - 轮询失败静默跳过本轮（ZCode 未运行/库忙时不 panic 不刷日志）。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, VecDeque};
use std::fs;
use std::io::{Read, Seek};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Mutex, OnceLock};
use std::thread;
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, WebviewUrl, WebviewWindowBuilder,
};

// ============================================================
// 常量
// ============================================================

/// 会话悬浮窗 label（与 capabilities 的 windows 白名单、事件目标一致）
pub const SESSION_HUD_WINDOW_LABEL: &str = "session-hud";
/// 会话快照事件（悬浮窗 listen，payload = HudSnapshot）
pub const SESSION_HUD_USAGE_EVENT: &str = "zbar://session-hud-usage";
/// 配置热推事件（悬浮窗 listen，payload = HudParams，设置变更时推送）
pub const SESSION_HUD_PARAMS_EVENT: &str = "zbar://session-hud-params";

/// 悬浮窗默认宽度（逻辑 px，基准尺寸；高度随会话条数自适应）
pub const HUD_DEFAULT_WIDTH: f64 = 300.0;
/// 宽度合法域（设置持久化前夹取，防脏值）
const HUD_WIDTH_RANGE: (f64, f64) = (240.0, 480.0);
/// 透明度默认值与合法域（前端经 CSS opacity 应用，见 HudParams）
const HUD_OPACITY_DEFAULT: f64 = 0.92;
const HUD_OPACITY_RANGE: (f64, f64) = (0.25, 1.0);

/// 活跃窗口档位（分钟）：5 / 10 / 30 / 0 = 不限；默认 10 分钟
pub const HUD_WINDOW_MINUTES_OPTIONS: [u32; 4] = [5, 10, 30, 0];
pub const HUD_WINDOW_MINUTES_DEFAULT: u32 = 10;
/// "不限"档的兜底窗口（分钟）：无时间窗过滤会扫全表且列表无限长，
/// 与 24h 内有活动的会话为限
const UNLIMITED_FALLBACK_MINUTES: u32 = 24 * 60;

/// 列表最多可见会话条数，超出折叠为"还有 N 个会话"
pub const MAX_VISIBLE_SESSIONS: usize = 5;
/// 活跃候选扫查上限（单信号源）：活跃查询按 last_at 降序 LIMIT，
/// 保证取到最新的若干会话；总数统计在该上限内饱和（折叠提示的上限）
const ACTIVITY_SCAN_LIMIT: i64 = 24;
/// model_usage 活跃扫查的行数上限（时间窗内最近 N 笔请求）：时间窗
/// （尤其"不限"档的 24h）内行数可能很大，内层以最近 N 笔为界保证
/// 查询成本有硬上限；超出极重负载漏掉的老会话属可接受窄边缘（最新
/// 会话——排序前几名——必然在内层结果中）
const MODEL_USAGE_SCAN_ROWS: i64 = 2000;
/// message 尾部扫查行数：与 usage_feed::collect_pending_user_ms 同款
/// 口径（最近消息必在 rowid 尾部，扫查成本与表大小无关，绝不全表扫）
const MESSAGE_TAIL_ROWS: i64 = 400;

/// 轮询拍间隔（毫秒）：1 秒一拍交替执行——DB 轮询拍隔拍一次（等效
/// 原 2 秒周期，查询内容不变），rollout 速度拍每拍执行（仅文件 stat +
/// 增量读 + 内存计算，stat 极轻）
const FEED_TICK_MS: u64 = 1000;
/// set_session_hud_config 等待主线程窗口操作完成的超时上限（与 pet
/// 同口径：仅事件循环异常退出时兜底，配置已先落盘不丢）
const HUD_WINDOW_OP_TIMEOUT: Duration = Duration::from_secs(10);

/// rollout 速度滑窗时长（毫秒）：窗口速度 = 各请求样本与窗口的重叠
/// 时长加权速率均值——大耗时请求按其历史速率摊入窗口，新行进入/旧行
/// 滑出都会让数字变化（对齐注入版的灵敏动态观感；窗口收窄到 4s，
/// 请求进入/滑出的数值变化更频繁明显）
const SPEED_WINDOW_MS: i64 = 4_000;
/// 速度样本保留时长（毫秒）：滑出窗口的样本仅作缓冲卫生清理（大于
/// 窗口即可，减少无效样本；内存上每树几十条，可忽略）
const SAMPLE_RETENTION_MS: i64 = 60_000;
/// 单样本摊销速率的时长下限（毫秒）：startedAt 与读取时刻几乎重合
///（极快请求/时钟抖动）时避免速率爆炸
const SAMPLE_MIN_SPAN_MS: i64 = 1_000;

/// 窗口布局常量（逻辑 px）：与 session-hud.html / session-hud-main.ts
/// 的 CSS 尺寸一一对应（头部拖动区 / 会话行 / 折叠提示行 / 底部留白），
/// Rust 侧据此按会话条数计算窗口高度。行高 56 = 三行制内容（项目行
/// 14 + 核心指标行 14 + 分解行 14 + 行距 3×2）+ 上下内边距 4×2
const HUD_HEADER_H: f64 = 30.0;
const HUD_ROW_H: f64 = 56.0;
const HUD_MORE_H: f64 = 20.0;
const HUD_BOTTOM_PAD: f64 = 8.0;
/// 空列表时的最小高度（仅头部 + "暂无活跃会话"提示态，不为 0 高）
const HUD_EMPTY_HEIGHT: f64 = 64.0;

/// 悬浮窗默认边距（px，逻辑坐标）：与 pet 同口径（macOS 16 / 其他 56）
#[cfg(target_os = "macos")]
const HUD_DEFAULT_MARGIN: f64 = 16.0;
#[cfg(not(target_os = "macos"))]
const HUD_DEFAULT_MARGIN: f64 = 56.0;
/// 默认位置在宠物窗默认区域之上的额外上移量（逻辑 px）：宠物窗默认
/// 也落在主显示器右下角（边长最大约 15% 屏高），HUD 上移让位，避免
/// 两窗出生即重叠
const HUD_DEFAULT_BOTTOM_EXTRA: f64 = 140.0;

// ============================================================
// 配置持久化（~/.zbar/session-hud.json，与项目其它配置同目录）
// ============================================================

/// 会话悬浮窗配置（皮肤页"会话悬浮窗"卡读写）。serde camelCase 与
/// 前端契约字段一一对应；`#[serde(default)]` 旧版文件缺字段按默认补齐，
/// 未知名（如改版前残留的 showContextBar）serde 默认忽略，旧配置文件
/// 解析不受影响，下次保存自然收敛到新字段集。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionHudConfig {
    /// 总开关：false = 关窗停轮询；true = 建窗 + 启轮询
    pub enabled: bool,
    /// 窗口左上角位置（逻辑坐标 x/y，拖动结束落盘，重启恢复）；
    /// None = 从未拖动过，创建时默认主显示器右下角（宠物窗上方）
    pub pos: Option<(f64, f64)>,
    /// 窗口宽度基准（逻辑 px；高度随会话条数自适应，不持久化）
    pub width: f64,
    /// 窗口不透明度（0.25~1.0，前端经 CSS opacity 应用到悬浮窗根节点）
    pub opacity: f64,
    /// 活跃窗口档位（分钟）：5/10/30，0 = 不限（仍有 24h 兜底）
    pub window_minutes: u32,
    /// 显示项：数据行（Σ 累计 / ↑ 输入 / ↓ 输出 / ⟲ 缓存读 / × 请求 /
    /// 速度 / TTFT 整行；关闭仅保留项目行）
    pub show_tokens: bool,
    /// 显示项：模型名
    pub show_model: bool,
}

impl Default for SessionHudConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            pos: None,
            width: HUD_DEFAULT_WIDTH,
            opacity: HUD_OPACITY_DEFAULT,
            window_minutes: HUD_WINDOW_MINUTES_DEFAULT,
            show_tokens: true,
            show_model: true,
        }
    }
}

impl SessionHudConfig {
    /// 把越界参数收敛到合法范围（保存前的防御，脏数据不落盘）：宽度/
    /// 透明度夹回合法域，档位归一到四个合法值（其余脏值一律回默认
    /// 10 分钟）
    pub fn clamped(mut self) -> Self {
        if !self.width.is_finite() {
            self.width = HUD_DEFAULT_WIDTH;
        }
        self.width = self.width.clamp(HUD_WIDTH_RANGE.0, HUD_WIDTH_RANGE.1);
        if !self.opacity.is_finite() {
            self.opacity = HUD_OPACITY_DEFAULT;
        }
        self.opacity = self
            .opacity
            .clamp(HUD_OPACITY_RANGE.0, HUD_OPACITY_RANGE.1);
        if !HUD_WINDOW_MINUTES_OPTIONS.contains(&self.window_minutes) {
            self.window_minutes = HUD_WINDOW_MINUTES_DEFAULT;
        }
        self
    }
}

fn hud_config_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("session-hud.json"))
}

/// 读取悬浮窗配置；文件不存在或内容损坏时静默返回默认值（首开无文件）。
pub fn load_session_hud_config() -> SessionHudConfig {
    let Ok(path) = hud_config_path() else {
        return SessionHudConfig::default();
    };
    fs::read_to_string(path)
        .ok()
        .and_then(|text| serde_json::from_str(&text).ok())
        .unwrap_or_default()
}

/// 保存悬浮窗配置：clamp 后原子性写入（先写临时文件再改名，与 pet
/// 同口径）。
pub fn save_session_hud_config(config: &SessionHudConfig) -> Result<(), String> {
    let path = hud_config_path()?;
    let json = serde_json::to_string_pretty(&config.clone().clamped())
        .map_err(|e| format!("序列化会话悬浮窗配置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入会话悬浮窗配置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("保存会话悬浮窗配置失败: {e}"))
}

// ============================================================
// Tauri 命令
// ============================================================

/// 读取会话悬浮窗配置（设置页卡片初始数据）。
#[tauri::command]
pub fn get_session_hud_config() -> Result<SessionHudConfig, String> {
    Ok(load_session_hud_config().clamped())
}

/// 保存并应用会话悬浮窗配置（改完即生效，无保存按钮）：
/// - 开关切换：enabled → 建窗 + 启轮询；!enabled → 停轮询 + 关窗；
/// - 其余字段变化：窗口存在时热推参数事件（页面即时应用透明度与
///   显示项；档位影响下一轮查询）。
///
/// async 命令 + run_on_main_thread + channel + 超时等待：与 pet.rs
/// 同款死锁防护（同步命令在 Windows 上占用主线程，建窗需要主线程
/// 事件循环处理消息，直接调窗口 API 会自等待死锁，pet.rs 有事故
/// 记载）。配置先落盘再动窗口：窗口操作失败配置不丢（下次启动
/// start_if_enabled 按 enabled 恢复）；关闭分支先停轮询再落盘，
/// Destroyed 复位路径（handle_session_hud_window_destroyed）读到
/// 已关的开关不再改写，幂等。
#[tauri::command]
pub async fn set_session_hud_config(
    config: SessionHudConfig,
    app: AppHandle,
) -> Result<SessionHudConfig, String> {
    let next = config.clamped();

    // 非开启态先停轮询（只是置位原子标志，线程安全；Destroyed 挂点
    // 还会再停一次，幂等）
    if !next.enabled {
        stop_feed();
    }

    // 先落盘再动窗口（见函数 doc：失败可恢复 + Destroyed 幂等前提）
    save_session_hud_config(&next)?;

    // 窗口创建/关闭投递主线程事件循环执行
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let cfg_main = next.clone();
    let app_main = app.clone();
    app.run_on_main_thread(move || {
        let outcome = if cfg_main.enabled {
            ensure_session_hud_window(&app_main, &cfg_main)
        } else if let Some(win) = app_main.get_webview_window(SESSION_HUD_WINDOW_LABEL) {
            let _ = win.close(); // 关窗失败不报错（窗口可能已被手动关闭）
            Ok(())
        } else {
            Ok(())
        };
        let _ = tx.send(outcome);
    })
    .map_err(|e| format!("投递会话悬浮窗操作到主线程失败: {e}"))?;

    match rx.recv_timeout(HUD_WINDOW_OP_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(e) => return Err(format!("会话悬浮窗操作超时: {e}")),
    }

    // 开启分支：建窗成功后才喂数据（失败即不启动，下次开可重试）
    if next.enabled {
        start_feed(app.clone());
    }

    Ok(next)
}

/// 应用启动挂点：开关开启时恢复窗口与轮询（未开启零开销）。
pub fn start_if_enabled(app: &AppHandle) {
    let cfg = load_session_hud_config().clamped();
    if !cfg.enabled {
        return;
    }
    if let Err(e) = ensure_session_hud_window(app, &cfg) {
        eprintln!("[zbar-session-hud] 恢复会话悬浮窗失败: {e}");
        return;
    }
    start_feed(app.clone());
}

// ============================================================
// 悬浮窗
// ============================================================

/// 最近一次窗口逻辑尺寸缓存（宽, 高）：设置卡重复应用（已存在分支）
/// 与窗口重建时按最后已知尺寸同步，避免热切换把活跃列表折叠回空态。
static LAST_SIZE: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();

fn last_size_slot() -> &'static Mutex<Option<(f64, f64)>> {
    LAST_SIZE.get_or_init(|| Mutex::new(None))
}

fn remember_size(size: (f64, f64)) {
    if let Ok(mut guard) = last_size_slot().lock() {
        *guard = Some(size);
    }
}

fn cached_size() -> Option<(f64, f64)> {
    last_size_slot().lock().ok().and_then(|g| *g)
}

/// 窗口代数计数器：窗口销毁/重建路径递增。feed 线程的变化检测 cache
/// 是线程局部变量——快速关-再开时旧线程可能被复用（sleep 窗口内未及
/// 感知 stop），若快照字节与销毁前一致则不会 emit，新页面会停在
/// "暂无活跃会话"空态直到数据变化。代数变化即清空 cache 强制重发
/// 一帧，保证任意路径重建窗口后一个轮询周期内新页面必收到快照。
static WINDOW_EPOCH: AtomicU64 = AtomicU64::new(0);

fn bump_window_epoch() {
    WINDOW_EPOCH.fetch_add(1, Ordering::Relaxed);
}

fn window_epoch() -> u64 {
    WINDOW_EPOCH.load(Ordering::Relaxed)
}

/// 窗口高度（逻辑 px，纯函数供单元测试复用）：头部 + 会话行 × 条数
/// + 折叠提示行 + 底部留白；空列表回空态最小高（显示"暂无活跃会话"，
/// 不为 0 高）。折叠提示行只在列表确实达到上限条数时出现（调用方传
/// total_active > MAX 的判定结果，此处再钳制一次互为双保险）。与
/// session-hud.html 的 CSS 尺寸常量一一对应。
fn hud_height(visible_rows: usize, folded: bool) -> f64 {
    if visible_rows == 0 {
        return HUD_EMPTY_HEIGHT;
    }
    let rows = visible_rows.min(MAX_VISIBLE_SESSIONS) as f64;
    let more = folded && visible_rows >= MAX_VISIBLE_SESSIONS;
    HUD_HEADER_H + rows * HUD_ROW_H + if more { HUD_MORE_H } else { 0.0 } + HUD_BOTTOM_PAD
}

/// 主显示器右下角默认位置（逻辑坐标）：右下留边距（Windows 含任务栏
/// 高度余量），并在宠物窗默认区域之上额外上移（避免两窗出生重叠）。
/// 显示器不可用（无头/异常驱动）时退回 (0, 0)。
fn default_bottom_right(mon: Option<&tauri::Monitor>, width: f64, height: f64) -> (f64, f64) {
    let Some(mon) = mon else {
        return (0.0, 0.0);
    };
    let scale = mon.scale_factor();
    let mon_w = mon.size().width as f64 / scale;
    let mon_h = mon.size().height as f64 / scale;
    (
        (mon_w - width - HUD_DEFAULT_MARGIN).max(0.0),
        (mon_h - height - HUD_DEFAULT_MARGIN - HUD_DEFAULT_BOTTOM_EXTRA).max(0.0),
    )
}

/// 确保 "session-hud" 悬浮窗存在并应用配置：已存在时仅同步尺寸与热推
/// 参数（防重复创建同名窗口）；不存在时创建（透明、无边框、置顶、
/// 不抢焦点、skipTaskbar、shadow 关闭，位置取持久化坐标或默认主显示
/// 器右下角）。必须在主线程的事件循环上下文调用（WebviewWindowBuilder
/// 的要求）：合法调用点是 setup 阶段（start_if_enabled）与
/// run_on_main_thread 投递的闭包（set_session_hud_config）。不能在同步
/// 命令主体里直接调——同步命令占用主线程，建窗等不到事件循环处理
/// 消息，自等待死锁（见 pet.rs 同名注释的事故记载）。
fn ensure_session_hud_window(app: &AppHandle, cfg: &SessionHudConfig) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(SESSION_HUD_WINDOW_LABEL) {
        // 已存在：按最后已知尺寸同步宽度（高度由轮询线程按会话条数
        // 自适应，这里不重算防折叠活跃列表），再热推参数
        let (w, h) = cached_size().unwrap_or((cfg.width, hud_height(0, false)));
        if (w - cfg.width).abs() > f64::EPSILON {
            let _ = win.set_size(LogicalSize::new(cfg.width, h));
            remember_size((cfg.width, h));
        }
        push_session_hud_params(app, cfg);
        return Ok(());
    }

    // 建窗初始尺寸：宽度恒取配置值（LAST_SIZE 缓存可能残留旧宽，不覆
    // 盖本次配置）；高度沿用最后已知高度（关后再开恢复原列表高度），
    // 无记录时空态最小高（首帧即"暂无活跃会话"，2 秒内数据到达后由
    // 轮询线程自适应）
    let (w, h) = (
        cfg.width,
        cached_size().map(|(_, h)| h).unwrap_or(hud_height(0, false)),
    );
    let mut builder = WebviewWindowBuilder::new(
        app,
        SESSION_HUD_WINDOW_LABEL,
        WebviewUrl::App("session-hud.html".into()),
    )
    .title("ZBar Session HUD")
    .inner_size(w, h)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    // Windows 钳宽修复（照搬 pet.rs）：tao 给所有窗口无条件带
    // WS_CAPTION，DefWindowProc 的 WM_GETMINMAXINFO 默认最小跟踪宽度
    // 会把小窗口出生即钳宽，min_inner_size(1,1) 在子类化后覆盖该值
    .min_inner_size(1.0, 1.0)
    // 不抢焦点（照搬 pet.rs）：focusable(false) 映射 WS_EX_NOACTIVATE
    // 从根上不激活，focused(false) 双保险；HUD 无键盘交互，拖动不需要
    // 激活，且绝不能从正在输入的会话里抢走焦点
    .focusable(false)
    .focused(false);

    // 位置：持久化逻辑坐标优先，无记录默认主显示器右下角（宠物窗上方）
    let mon = app.primary_monitor().ok().flatten();
    let (x, y) = match cfg.pos {
        Some(p) => p,
        None => default_bottom_right(mon.as_ref(), w, h),
    };
    builder = builder.position(x, y);

    // 平台差异处理（照搬 pet.rs）：
    // macOS：显式清空窗口背景，避免 WebView 默认背景把透明窗口盖成纯白；
    // Windows：隐藏建窗 → 显式 set_size 收回请求尺寸（规避出生时刻的
    // 最小跟踪宽度钳制）→ 显示
    #[cfg(target_os = "macos")]
    {
        let win = builder
            .build()
            .map_err(|e| format!("创建会话悬浮窗失败: {e}"))?;
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    }
    #[cfg(not(target_os = "macos"))]
    {
        let win = builder
            .visible(false)
            .build()
            .map_err(|e| format!("创建会话悬浮窗失败: {e}"))?;
        let _ = win.set_size(LogicalSize::new(w, h));
        let _ = win.show();
    }
    remember_size((w, h));
    // 新窗口诞生即递增代数：feed 线程（含被复用的旧线程）下一拍清空
    // 变化检测缓存，向新页面强制重发一帧快照（首帧数据保障）
    bump_window_epoch();

    // 首帧参数：页面加载后也会主动 get_session_hud_config，这里推送
    // 保证先到（双通道幂等）
    push_session_hud_params(app, cfg);
    Ok(())
}

/// 向悬浮窗推送当前显示参数（透明度 + 显示项 + 活跃档位）。透明度由
/// 页面经 CSS opacity 应用（窗口级 set_opacity 平台差异大，CSS 路径
/// 跨平台一致且作用于内容层）。
fn push_session_hud_params(app: &AppHandle, cfg: &SessionHudConfig) {
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HudParams {
        opacity: f64,
        window_minutes: u32,
        show_tokens: bool,
        show_model: bool,
    }
    let _ = app.emit_to(
        SESSION_HUD_WINDOW_LABEL,
        SESSION_HUD_PARAMS_EVENT,
        HudParams {
            opacity: cfg.opacity,
            window_minutes: cfg.window_minutes,
            show_tokens: cfg.show_tokens,
            show_model: cfg.show_model,
        },
    );
}

// ============================================================
// 窗口位置持久化（Moved 事件节流落盘，与 pet.rs 同款）
// ============================================================

/// 最近一次窗口位置（逻辑坐标）：Moved 事件高频触发（拖动时连续），
/// 先写内存，按节流间隔落盘，窗口销毁时冲刷最终值
static HUD_POS: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
/// 上次位置落盘时刻（毫秒），0 = 从未落盘
static HUD_POS_SAVED_AT: AtomicU64 = AtomicU64::new(0);
/// 位置落盘节流间隔（毫秒）：拖动结束后最迟 1 秒内持久化
const HUD_POS_SAVE_THROTTLE_MS: u64 = 1000;

fn hud_pos_slot() -> &'static Mutex<Option<(f64, f64)>> {
    HUD_POS.get_or_init(|| Mutex::new(None))
}

/// 悬浮窗 Moved 事件挂点（lib.rs 的 on_window_event 转发）：物理坐标
/// 转逻辑坐标写内存，节流合并进 session-hud.json（不动其它字段）。
pub fn handle_session_hud_window_moved(win: &tauri::Window, pos: tauri::PhysicalPosition<i32>) {
    let scale = win.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return;
    }
    let logical = (pos.x as f64 / scale, pos.y as f64 / scale);
    {
        let Ok(mut guard) = hud_pos_slot().lock() else {
            return;
        };
        *guard = Some(logical);
    }
    // 节流落盘：拖动期间每秒最多一次写盘
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let last = HUD_POS_SAVED_AT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < HUD_POS_SAVE_THROTTLE_MS {
        return;
    }
    HUD_POS_SAVED_AT.store(now, Ordering::Relaxed);
    persist_hud_pos(logical);
}

/// 悬浮窗 Destroyed 事件挂点：冲刷最终位置（无节流）+ 停轮询 + 开关
/// 复位（窗口没了 = 功能关闭，防面板显示与实况脱节；用户 alt+F4 等
/// 旁路关闭后开关能如实回读为关）。
pub fn handle_session_hud_window_destroyed(_app: &AppHandle) {
    stop_feed();
    // 递增窗口代数：销毁后快速重开时旧 feed 线程可能被复用（sleep 窗口
    // 内未及感知 stop），代数变化令其清空变化检测缓存重发一帧，新页面
    // 不停留在假空态（见 WINDOW_EPOCH 注释）
    bump_window_epoch();
    let pos = hud_pos_slot().lock().ok().and_then(|guard| *guard);
    if let Some(p) = pos {
        HUD_POS_SAVED_AT.store(
            chrono::Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
        persist_hud_pos(p);
    }
    // 开关复位：仅开启态（旁路关闭如实反映）；set_session_hud_config
    // 的正常关闭流程已先落盘新开关，此处读到的已关不动（幂等）
    let mut cfg = load_session_hud_config();
    if cfg.enabled {
        cfg.enabled = false;
        let _ = save_session_hud_config(&cfg);
    }
}

/// 把位置合并进 session-hud.json（保留其它字段）。落盘失败静默（下次
/// Moved 再试）。
fn persist_hud_pos(pos: (f64, f64)) {
    let mut cfg = load_session_hud_config();
    if cfg.pos == Some(pos) {
        return;
    }
    cfg.pos = Some(pos);
    let _ = save_session_hud_config(&cfg);
}

// ============================================================
// 独立轮询器（普通 thread + flag 模式，沿用 pet.rs / usage_feed 惯例）
// ============================================================

static FEED_STOP: AtomicBool = AtomicBool::new(false);
static FEED_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();

fn feed_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    FEED_HANDLE.get_or_init(|| Mutex::new(None))
}

/// 启动轮询线程（悬浮窗开启挂点调用）。已在运行时为幂等 no-op。
/// 启动失败仅放弃本功能（不 panic 不阻塞调用方），下次开窗可重试。
pub fn start_feed(app: AppHandle) {
    let mut guard = match feed_handle().lock() {
        Ok(g) => g,
        Err(poisoned) => poisoned.into_inner(),
    };
    if guard.as_ref().is_some_and(|h| !h.is_finished()) {
        FEED_STOP.store(false, Ordering::Relaxed);
        return;
    }
    FEED_STOP.store(false, Ordering::Relaxed);
    if let Ok(h) = thread::Builder::new()
        .name("zbar-session-hud-feed".into())
        .spawn(move || feed_loop(app))
    {
        *guard = Some(h);
    }
}

/// 停止轮询线程（悬浮窗关闭挂点调用）。仅置位停止标记，不 join；
/// 线程完成当前查询周期（含 DB busy 等待）后退出。
pub fn stop_feed() {
    FEED_STOP.store(true, Ordering::Relaxed);
}

fn feed_loop(app: AppHandle) {
    // 变化检测缓存（快照序列化字节）与最后同步的窗口逻辑尺寸
    let mut cache: Option<String> = None;
    let mut last_size: Option<(f64, f64)> = None;
    // 线程启动时的窗口代数基线：重建窗口（Destroyed/建窗路径递增全局
    // 计数）后清空变化检测缓存，强制向新页面重发一帧快照（见
    // WINDOW_EPOCH 注释的假空态场景）
    let mut epoch = window_epoch();
    // rollout 旁路速度监控器与最后快照（速度拍在其上原地更新 speed）
    let mut monitor = RolloutMonitor::default();
    let mut last_snapshot: Option<HudSnapshot> = None;
    let mut last_fallback: BTreeMap<String, Option<f64>> = BTreeMap::new();
    // 拍交替标志：首拍必为 DB 拍（速度拍依赖已有快照）
    let mut db_tick = false;
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        let now_epoch = window_epoch();
        if now_epoch != epoch {
            epoch = now_epoch;
            cache = None; // 新窗口尚未收到任何数据，下一拍必重发
        }
        // 1 秒一拍交替：DB 轮询拍隔拍一次（等效原 2 秒周期，查询内容
        // 不变），rollout 速度拍每拍执行（仅文件 stat + 增量读 + 内存
        // 计算）
        db_tick = !db_tick;
        if db_tick {
            poll_db(
                &app,
                &mut cache,
                &mut last_size,
                &mut monitor,
                &mut last_snapshot,
                &mut last_fallback,
            );
        } else {
            poll_speed(
                &app,
                &mut cache,
                &mut monitor,
                &mut last_snapshot,
                &last_fallback,
            );
        }
        // 分段睡眠：sleep 期间可及时感知 stop（拍内处理期间不响应）
        for _ in 0..(FEED_TICK_MS / 100) {
            if FEED_STOP.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// DB 轮询拍（隔拍一次，等效原 2 秒周期）：读配置 → 查库构造会话快照
/// 与会话树 → 同步 rollout 游标并增量读新样本 → 速度展示决策 → 内容
/// 变化才 emit → 高度变化才经主线程调窗口尺寸。任何失败静默跳过本轮
///（下个周期重试），不 panic 不刷日志——ZCode 未运行/无库时事件停流，
/// 悬浮窗保持最后内容。
fn poll_db(
    app: &AppHandle,
    cache: &mut Option<String>,
    last_size: &mut Option<(f64, f64)>,
    monitor: &mut RolloutMonitor,
    last_snapshot: &mut Option<HudSnapshot>,
    last_fallback: &mut BTreeMap<String, Option<f64>>,
) {
    let result = (|| -> Result<(), String> {
        let cfg = load_session_hud_config().clamped();
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let (mut snapshot, trees) =
            collect_session_snapshot(&conn, cfg.window_minutes, now_ms)?;

        // rollout 旁路速度：同步游标 → 增量读新完成请求 → 展示决策。
        // 仅覆盖 speed 字段；DB 最近完成请求口径保留为 fallback（rollout
        // 全缺失/全解析失败时回退）。累计字段与 rollout 无关（防双计）。
        monitor.sync_trees(&trees, rollout_dir().as_deref());
        monitor.ingest(now_ms);
        last_fallback.clear();
        for s in &mut snapshot.sessions {
            last_fallback.insert(s.session_id.clone(), s.speed);
            if let Some(tree) = monitor.tree_mut(&s.session_id) {
                // 新一轮生成开始（message 信号 false→true 跳变）：清除
                // 上一轮保持值，速度位回到"测算中"态（生成中且窗口无
                // 新样本 → None → 前端呼吸占位）
                if s.msg_generating && !tree.msg_gen_active {
                    tree.begin_new_round();
                }
                tree.msg_gen_active = s.msg_generating;
                s.speed = tree.display_speed(s.generating, s.speed, now_ms);
            }
        }

        // 变化检测：会话序列化字节不变则不 emit（与 pet.rs 同款手法，
        // 省无谓 IPC 与页面重渲染）
        let key = serde_json::to_string(&snapshot)
            .map_err(|e| format!("序列化会话快照失败: {e}"))?;
        if cache.as_deref() != Some(key.as_str()) {
            *cache = Some(key);
            let _ = app.emit_to(SESSION_HUD_WINDOW_LABEL, SESSION_HUD_USAGE_EVENT, &snapshot);
        }
        *last_snapshot = Some(snapshot);

        // 高度自适应：会话条数（含折叠态）变化 → 窗口高度随之调整。
        // 必须在主线程操作窗口（轮询线程非主线程，直接调窗口 API 会
        // panic）；投递失败静默（下轮条数变化时再试）。宽度同步配置
        // 值（设置卡改宽度即热生效）。
        let visible = last_snapshot
            .as_ref()
            .map(|s| s.sessions.len())
            .unwrap_or(0)
            .min(MAX_VISIBLE_SESSIONS);
        let total_active = last_snapshot
            .as_ref()
            .map(|s| s.total_active)
            .unwrap_or(0);
        let size = (
            cfg.width,
            hud_height(visible, total_active > MAX_VISIBLE_SESSIONS),
        );
        if last_size.map(|s| s != size).unwrap_or(true) {
            *last_size = Some(size);
            remember_size(size);
            // 两个句柄：一个供方法调用（借用于调用期间），一个供闭包捕获
            let app_task = app.clone();
            let app_call = app.clone();
            let _ = app_call.run_on_main_thread(move || {
                if let Some(win) = app_task.get_webview_window(SESSION_HUD_WINDOW_LABEL) {
                    let _ = win.set_size(LogicalSize::new(size.0, size.1));
                }
            });
        }
        Ok(())
    })();
    let _ = result; // 静默跳过本轮（库被锁超时/文件缺失等瞬态），下个周期重试
}

/// 速度拍（与 DB 拍交替，1 秒一次）：仅 rollout 增量读 + 窗口速度重算，
/// 不查库。速度变化并入快照变化检测——速度字段每秒可刷新，其余字段
/// 仍随 DB 拍 2 秒更新，内容不变不 emit。
fn poll_speed(
    app: &AppHandle,
    cache: &mut Option<String>,
    monitor: &mut RolloutMonitor,
    last_snapshot: &mut Option<HudSnapshot>,
    last_fallback: &BTreeMap<String, Option<f64>>,
) {
    let Some(snapshot) = last_snapshot.as_mut() else {
        return; // 首个 DB 拍尚未成功，无内容可更新
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    monitor.ingest(now_ms);
    for s in &mut snapshot.sessions {
        if let Some(tree) = monitor.tree_mut(&s.session_id) {
            let fallback = last_fallback.get(&s.session_id).copied().flatten();
            s.speed = tree.display_speed(s.generating, fallback, now_ms);
        }
    }
    // 速度变化才 emit（与 DB 拍共用变化检测缓存）
    if let Ok(key) = serde_json::to_string(&snapshot) {
        if cache.as_deref() != Some(key.as_str()) {
            *cache = Some(key);
            let _ = app.emit_to(SESSION_HUD_WINDOW_LABEL, SESSION_HUD_USAGE_EVENT, &*snapshot);
        }
    }
}

// ============================================================
// 会话快照构造（纯逻辑拆分便于单元测试，不依赖真实 ~/.zcode）
// ============================================================

/// 推给悬浮窗的单会话摘要（紧凑标量结构，只含前端展示所需字段，
/// 不透出原始行数据——与 pet.rs 的 PetTurnBrief 裁剪同思路）。
/// 口径说明：行主体是"会话树"（主会话 + 全部子代理，注入版 V9 合计
/// 口径，子代理永不单独成行防双计）；累计字段 ↑/↓/⟲/Σ/× 均为树内
/// model_usage 合计。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HudSessionBrief {
    /// 主会话 id（前端暂不展示，调试/扩展预留）
    pub session_id: String,
    /// 会话短标识（去 "sess_" 前缀后的前 6 位，用于区分同项目多会话）
    pub short: String,
    /// 项目名（session.directory 的最后一段文件夹名；目录缺失时回退
    /// "#短标识"）
    pub project: String,
    /// 模型集合（会话树时间窗内 distinct model_id，按最近使用降序逗号
    /// 拼接，注入版 models 口径；前端显示首个 + 计数）；无请求为 null
    pub models: Option<String>,
    /// 窗口内有活动的子代理数（>0 时前端显示 ⟡n 徽标）
    pub sub_count: usize,
    /// 状态：true = 生成中（树内任一成员最新一轮未完成），false = 空闲
    pub generating: bool,
    /// 窗口内最近活动时刻（毫秒，主会话与子代理取 max，排序键）
    pub last_active_at: i64,
    /// 全生命周期累计：↑ 非缓存输入（逐笔 max(0, input − cache_read)
    /// 汇总，注入版"保守取小值"口径）
    pub in_tokens: i64,
    /// 全生命周期累计：↓ 输出
    pub out_tokens: i64,
    /// 全生命周期累计：⟲ 缓存读
    pub cache_read: i64,
    /// 全生命周期累计：Σ = ↑ + ↓ + ⟲（注入版 V15 口径）
    pub total: i64,
    /// 全生命周期累计：× 模型请求笔数（model_usage 行数，每行一笔
    /// 请求，与注入版 Σ req 口径等价且更实时）
    pub req_count: i64,
    /// 动态速度（t/s，"该会话最近一次生成的速率"语义，是否在生成由
    /// 状态点表达）：优先 rollout 旁路滑动窗口速度（生成中实时更新；
    /// 长请求无新行与空闲均保持最后非零不归 0；新一轮刚开始 → null
    /// 显示"测算中"呼吸占位），rollout 全缺失回退最近一笔完成请求
    /// 口径（同值不归 0）；无数据为 null → "–"
    pub speed: Option<f64>,
    /// message 信号驱动的生成中标记（serde skip 不进 payload，仅供
    /// poll_db 在 rollout 保持值上做"新一轮重置"）
    #[serde(skip)]
    pub msg_generating: bool,
    /// 最近一笔完成请求的 TTFT（time_to_first_token_ms，静态参考）；
    /// None → 前端显示 "–"
    pub ttft_ms: Option<i64>,
}

/// 推给悬浮窗的会话快照（事件 payload）
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HudSnapshot {
    /// 数据契约版本
    pub v: u8,
    /// 窗口内活跃会话总数（含未展示的，前端折叠提示"还有 N 个"）
    pub total_active: usize,
    /// 会话列表（按最近活动倒序，至多 MAX_VISIBLE_SESSIONS 条）
    pub sessions: Vec<HudSessionBrief>,
}

/// 查询库并构造会话快照：活跃判定（时间窗内 model_usage ∪ message）
/// → 子代理归并主会话 → 取最新若干条 → 逐会话树聚合。任一查询失败
/// 整体返回 Err（调用方静默跳过本轮，避免列表闪空）。同时返回展示中
/// 的会话树成员清单（供 rollout 旁路速度源定位文件集合）。
pub(crate) fn collect_session_snapshot(
    conn: &Connection,
    window_minutes: u32,
    now_ms: i64,
) -> Result<(HudSnapshot, Vec<SessionTree>), String> {
    // 老版本库无核心列：返回空快照（悬浮窗显示空态，不报错）
    if !has_table(conn, "model_usage")
        || !crate::db::has_column(conn, "model_usage", "session_id")
        || !crate::db::has_column(conn, "model_usage", "started_at")
    {
        return Ok((
            HudSnapshot {
                v: 1,
                total_active: 0,
                sessions: Vec::new(),
            },
            Vec::new(),
        ));
    }

    // 有效窗口：0（不限）也有 24h 兜底上限（防全表扫 + 列表无限长）
    let minutes = if window_minutes == 0 {
        UNLIMITED_FALLBACK_MINUTES
    } else {
        window_minutes
    };
    let window_start = now_ms - minutes as i64 * 60_000;

    // 1) 活跃候选并集：model_usage（started_at 时间窗过滤，走
    //    model_usage_started_model_idx 前缀索引，绝不全表扫）∪ message
    //    （rowid 尾部扫查，见 MESSAGE_TAIL_ROWS）。两路各 LIMIT 候选
    //    上限，按 last_at 降序保证取到最新会话。
    let mut activity: BTreeMap<String, i64> = BTreeMap::new();
    // 会话 → 最新 user 消息时刻（生成中判定驱动信号，见 collect_
    // session_brief 的 msg 分支；仅尾部扫查窗口内有效，与活跃判定一致）
    let mut msg_times: BTreeMap<String, i64> = BTreeMap::new();
    {
        // 内层子查询先按 started_at 索引倒序取窗口内最近 N 笔（强制
        // SEARCH started_at>? 范围扫；直接对全表 GROUP BY 会被计划器
        // 选成 model_usage_session_turn_idx 全索引扫 + 逐行回表），外层
        // 再对小结果集分组取每会话最近活动时刻
        let mut stmt = conn
            .prepare(
                "SELECT session_id, MAX(started_at) FROM \
                 (SELECT session_id, started_at FROM model_usage \
                  WHERE started_at >= ?1 ORDER BY started_at DESC LIMIT ?2) \
                 GROUP BY session_id ORDER BY 2 DESC LIMIT ?3",
            )
            .map_err(|e| format!("准备会话活跃查询失败: {e}"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![
                    window_start,
                    MODEL_USAGE_SCAN_ROWS,
                    ACTIVITY_SCAN_LIMIT
                ],
                |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?)),
            )
            .map_err(|e| format!("读取会话活跃失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取会话活跃失败: {e}"))?;
        for (id, at) in rows {
            merge_activity(&mut activity, id, at);
        }
    }
    if has_table(conn, "message")
        && crate::db::has_column(conn, "message", "session_id")
        && crate::db::has_column(conn, "message", "time_created")
        && crate::db::has_column(conn, "message", "data")
    {
        // message 的 time_created 无独立时间索引，按时间过滤会全表扫；
        // 沿用 usage_feed 的 rowid 尾部扫口径（最近消息必在尾部）。
        // 活跃信号不区分角色（用户发消息与助手回复落库都是真实活动）；
        // user 消息时刻单独记录（json_extract 解析角色，尾部 400 行
        // 成本可忽略）——仅 user 消息（实测全部为真实用户提示词）可
        // 作为"新轮已开始"信号，助手消息落库与请求完成同时序，会误判
        let mut stmt = conn
            .prepare(
                "SELECT session_id, MAX(time_created), \
                        MAX(CASE WHEN role = 'user' THEN time_created END) \
                 FROM (SELECT session_id, time_created, \
                              json_extract(data, '$.role') AS role \
                       FROM message ORDER BY rowid DESC LIMIT ?1) \
                 WHERE time_created >= ?2 GROUP BY session_id ORDER BY 2 DESC",
            )
            .map_err(|e| format!("准备消息活跃查询失败: {e}"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![MESSAGE_TAIL_ROWS, window_start],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, i64>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                    ))
                },
            )
            .map_err(|e| format!("读取消息活跃失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取消息活跃失败: {e}"))?;
        for (id, at, user_at) in rows {
            merge_activity(&mut activity, id.clone(), at);
            if let Some(ua) = user_at.filter(|t| *t > 0) {
                msg_times.insert(id, ua);
            }
        }
    }

    // 2) 会话树归并（注入版 V9 合计口径）：候选中的子代理（parent_id
    //    非空）不再单独成行，其活动时刻归并到主会话（last_active 取
    //    max）——主会话自身空闲但子代理在跑时整行不消失。parent_id 列
    //    缺失（老版本库）降级为无归并无过滤（所有候选视作主会话）。
    //    候选元信息逐条 PK 点查（session 表主键，候选 ≤ 2×SCAN 上限）。
    let has_parent_col = has_table(conn, "session")
        && crate::db::has_column(conn, "session", "id")
        && crate::db::has_column(conn, "session", "directory")
        && crate::db::has_column(conn, "session", "parent_id");

    let mut candidates: Vec<(String, i64)> = activity
        .iter()
        .map(|(id, at)| (id.clone(), *at))
        .collect();
    // 按最近活动倒序（正在生成的会话天然在最顶部）
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // 主会话槽位：directory + 归并后的最近活动时刻（子代理候选落进
    // 父会话槽位，父会话自身候选再落时两者合并）
    let mut mains: BTreeMap<String, (Option<String>, i64)> = BTreeMap::new();
    // 仅由子代理归并引入的父会话（不在候选里，需补查目录）
    let mut merged_only: Vec<String> = Vec::new();
    for (id, last_at) in &candidates {
        // has_parent_col=false（老版本库缺列）不查元信息：directory 与
        // parent 一律 None（无归并无过滤的降级现状）
        let (directory, parent) = if has_parent_col {
            session_meta(conn, id)?
        } else {
            (None, None)
        };
        if let Some(parent_id) = parent {
            // 子代理：归并活动到主会话槽位（父会话不在候选也会被带入）
            if !mains.contains_key(&parent_id) {
                merged_only.push(parent_id.clone());
                mains.insert(parent_id.clone(), (None, 0));
            }
            let slot = mains.get_mut(&parent_id).expect("父会话槽位已就绪");
            if *last_at > slot.1 {
                slot.1 = *last_at;
            }
        } else {
            match mains.get_mut(id) {
                // 槽位已被其子代理先创建：补目录、活动取 max
                Some(slot) => {
                    slot.0 = directory;
                    if *last_at > slot.1 {
                        slot.1 = *last_at;
                    }
                }
                None => {
                    mains.insert(id.clone(), (directory, *last_at));
                }
            }
        }
    }
    // 仅由归并引入的父会话：补查目录（行缺失保持 None → "#短标识"）
    for pid in &merged_only {
        let (directory, _) = session_meta(conn, pid)?;
        if let Some(slot) = mains.get_mut(pid) {
            slot.0 = directory;
        }
    }

    let mut ordered: Vec<(String, Option<String>, i64)> = mains
        .into_iter()
        .map(|(id, (dir, at))| (id, dir, at))
        .collect();
    ordered.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    // 3) 只对将展示的条目做会话树聚合（行数最多的部分查询到此为止）。
    //    会话树成员 = 自身 + 全部子代理（session_parent_idx 等值查）；
    //    徽标计数 = 树内窗口内有活动的子代理数（活跃表 ∩ 子代理集合）。
    let total_active = ordered.len();
    let mut sessions: Vec<HudSessionBrief> = Vec::new();
    let mut trees: Vec<SessionTree> = Vec::new();
    for (session_id, directory, last_at) in ordered {
        if sessions.len() >= MAX_VISIBLE_SESSIONS {
            continue;
        }
        let mut members = vec![session_id.clone()];
        let mut active_subs = 0usize;
        if has_parent_col {
            let mut stmt = conn
                .prepare("SELECT id FROM session WHERE parent_id = ?1")
                .map_err(|e| format!("读取子代理会话失败: {e}"))?;
            let children = stmt
                .query_map([&session_id], |row| row.get::<_, String>(0))
                .map_err(|e| format!("读取子代理会话失败: {e}"))?
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| format!("读取子代理会话失败: {e}"))?;
            active_subs = children
                .iter()
                .filter(|c| activity.contains_key(*c))
                .count();
            members.extend(children);
        }
        // 树口径的最新 user 消息时刻（生成中判定的 message 驱动信号）
        let tree_last_msg = members
            .iter()
            .filter_map(|m| msg_times.get(m))
            .copied()
            .max()
            .unwrap_or(0);
        trees.push(SessionTree {
            root: session_id.clone(),
            members: members.clone(),
        });
        sessions.push(collect_session_brief(
            conn,
            &members,
            &session_id,
            window_start,
            directory.as_deref(),
            last_at,
            active_subs,
            tree_last_msg,
        )?);
    }

    Ok((
        HudSnapshot {
            v: 1,
            total_active,
            sessions,
        },
        trees,
    ))
}

/// 活跃时刻合并（同一会话两信号取更晚者）
fn merge_activity(map: &mut BTreeMap<String, i64>, session_id: String, at: i64) {
    map.entry(session_id)
        .and_modify(|cur| {
            if at > *cur {
                *cur = at;
            }
        })
        .or_insert(at);
}

/// 会话元信息：项目目录（directory 最后一段在调用方解析，此处回传原值）
/// 与父会话 id（非空 = 子代理会话）。仅在 session 表含 directory/
/// parent_id 列时调用（调用方以 has_parent_col 守卫）；行缺失返回
/// (None, None)。
fn session_meta(
    conn: &Connection,
    session_id: &str,
) -> Result<(Option<String>, Option<String>), String> {
    match conn.query_row(
        "SELECT directory, parent_id FROM session WHERE id = ?1",
        [session_id],
        |row| {
            Ok((
                row.get::<_, Option<String>>(0)?,
                row.get::<_, Option<String>>(1)?,
            ))
        },
    ) {
        Ok((directory, parent)) => Ok((directory, parent)),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok((None, None)),
        Err(e) => Err(format!("读取会话信息失败: {e}")),
    }
}

/// 会话树（主会话 + 全部子代理）摘要：model_usage 按成员 IN 等值查
///（model_usage_session_turn_idx 前缀索引，每成员一次索引定位）单趟
/// 扫查完成全部统计。合计口径对齐注入版会话条：↑ = Σ max(0, input −
/// cache_read)，↓ = Σ output，⟲ = Σ cache_read，Σ = ↑+↓+⟲，× = 行数
///（每行一笔请求）；子代理并入主会话行、永不单独成行（V9 防双计）。
/// 另产出：模型集合（时间窗内 distinct，按最近使用降序）、生成中判定
///（树内任一成员最新轮未完成即生成中）、动态速度/TTFT（最近一笔完成
/// 请求；生成中显示计算值、空闲归 0.0）。
fn collect_session_brief(
    conn: &Connection,
    members: &[String],
    session_id: &str,
    window_start: i64,
    directory: Option<&str>,
    last_active_at: i64,
    active_subs: usize,
    tree_last_msg_ms: i64,
) -> Result<HudSessionBrief, String> {
    // 列探测降级（老版本库缺列按常量 0/NULL 补齐，与 usage_feed 同款）
    let has = |col: &str| crate::db::has_column(conn, "model_usage", col);
    let has_status = has("status");
    let in_expr = num_expr(has("input_tokens"), "input_tokens");
    let out_expr = num_expr(has("output_tokens"), "output_tokens");
    let cr_expr = num_expr(has("cache_read_input_tokens"), "cache_read_input_tokens");
    let turn_expr = if has("turn_id") { "turn_id" } else { "NULL" };
    let status_expr = if has_status { "status" } else { "NULL" };
    let dur_expr = opt_expr(has("duration_ms"), "duration_ms");
    let ttft_expr = opt_expr(has("time_to_first_token_ms"), "time_to_first_token_ms");

    // 树成员 IN 等值查：占位数与成员数一致（成员 ≤ 1 + 子代理数，
    // 个位数量级）
    let placeholders = vec!["?"; members.len()].join(", ");
    let sql = format!(
        "SELECT session_id, {in_expr}, {out_expr}, {cr_expr}, \
                COALESCE(model_id, ''), {turn_expr}, {status_expr}, started_at, \
                {dur_expr}, {ttft_expr} \
         FROM model_usage WHERE session_id IN ({placeholders})"
    );
    let mut stmt = conn
        .prepare(&sql)
        .map_err(|e| format!("准备会话聚合查询失败: {e}"))?;
    let mut rows = stmt
        .query(rusqlite::params_from_iter(members.iter()))
        .map_err(|e| format!("查询会话聚合失败: {e}"))?;

    // 单趟聚合：逐行累加（↑ 按行 clamp 负值——个别行的 cache_read 大于
    // input 时取 0，注入版"保守取小值"同款）+ 模型集合（时间窗）+ 成员
    // 最新一笔跟踪（生成中判定）+ 最近一笔完成请求（速度/TTFT）
    let mut agg = SessionAgg::default();
    while let Some(row) = rows
        .next()
        .map_err(|e| format!("遍历会话聚合失败: {e}"))?
    {
        // 行内取值错误统一转 String（rusqlite::Error 无 From<String> 通道）
        let cell = |i: usize| -> Result<i64, String> {
            row.get(i).map_err(|e| format!("读取会话聚合失败: {e}"))
        };
        let opt_cell = |i: usize| -> Result<Option<i64>, String> {
            row.get(i).map_err(|e| format!("读取会话聚合失败: {e}"))
        };
        let take_text = |i: usize| -> Result<Option<String>, String> {
            row.get(i).map_err(|e| format!("读取会话聚合失败: {e}"))
        };
        let member: String = row.get(0).map_err(|e| format!("读取会话聚合失败: {e}"))?;
        let input = cell(1)?;
        let output = cell(2)?;
        let cache_read = cell(3)?;
        let model: String = row.get(4).map_err(|e| format!("读取会话聚合失败: {e}"))?;
        let turn: Option<String> = take_text(5)?;
        let status: Option<String> = take_text(6)?;
        let started_at = cell(7)?;
        let dur: Option<i64> = opt_cell(8)?;
        let ttft: Option<i64> = opt_cell(9)?;

        // 合计（子代理并入主会话行；防双计靠"子代理永不单独成行"）
        agg.plain_in += (input - cache_read).max(0);
        agg.out_tokens += output;
        agg.cache_read += cache_read;
        agg.req_count += 1;

        // 模型集合：时间窗内 distinct，记录各模型最近使用时刻（排序用）
        if started_at >= window_start && !model.is_empty() {
            agg.models
                .entry(model.clone())
                .and_modify(|t| {
                    if started_at > *t {
                        *t = started_at;
                    }
                })
                .or_insert(started_at);
        }

        // 最近一笔完成请求判定（速度/TTFT 数据源；status 列缺失按全完成
        // 降级——老库 model_usage 完成即落行，无 running 行）。须在
        // turn/status 被 tail 跟踪移动之前取值
        let is_completed = !has_status || status.as_deref() == Some("completed");

        // 成员最新一笔（生成中判定按成员跟踪，树内取或）。守卫分支保证
        // turn/status 的移动在各自分支内无条件发生（条件内移动会触发
        // maybe-moved 报错）
        match agg.tails.get_mut(&member) {
            Some(tail) if started_at >= tail.latest_at => {
                tail.latest_at = started_at;
                tail.turn = turn;
                tail.status = status;
            }
            Some(_) => {}
            None => {
                agg.tails.insert(
                    member,
                    MemberTail {
                        latest_at: started_at,
                        turn,
                        status,
                    },
                );
            }
        }

        if is_completed && started_at >= agg.latest_completed_at {
            agg.latest_completed_at = started_at;
            agg.latest_completed = Some((output, dur, ttft));
        }
    }

    // 生成中判定（树内取或）：任一成员最新一轮未完成即生成中。
    // turn_usage 已有该轮且非 running = 完成（completed/error/cancelled
    // 都是终态）→ 空闲；无该轮行 = 轮仍在进行（runs 同款口径），但最新
    // 请求本身 error/cancelled 时失败轮可能永不落 turn 行，判空闲防状
    // 态点永久卡"生成中"。
    let mut generating = false;
    for (member, tail) in &agg.tails {
        let unfinished = match tail.turn.as_deref() {
            Some(turn) if !turn.is_empty() => match turn_status(conn, member, turn)? {
                Some(status) => status == "running",
                None => !matches!(
                    tail.status.as_deref(),
                    Some("error") | Some("cancelled")
                ),
            },
            _ => false, // 无 turn_id（老版本库/无请求）无法判定 → 不贡献
        };
        if unfinished {
            generating = true;
            break;
        }
    }
    // message 信号驱动（防"发消息后首笔请求完成前 10~60 秒无反馈"）：
    // 树内最新 user 消息晚于树内最新请求开始时刻（MemberTail）→ 新轮
    // 已开始而首笔请求尚未落库 → 生成中。角色限定 user（助手消息落库
    // 与请求完成同时序，会把刚完成的轮误判回生成中）；窗口档位限制已在
    // 活跃发现阶段生效；无消息（0）不触发。首笔完成请求落库后其
    // started_at 必然晚于消息时刻，判定自动交还原轮级口径。
    let tree_latest_req = agg.tails.values().map(|t| t.latest_at).max().unwrap_or(0);
    let msg_generating = tree_last_msg_ms > tree_latest_req;
    if msg_generating {
        generating = true;
    }

    // DB 口径速度（rollout 缺失/未观测时的 fallback）："最近生成速率"
    // 语义，生成中与空闲同值不归 0（空闲保持显示该值，是否在生成由
    // 状态点表达）。TTFT 恒为最近一笔完成请求的值（静态参考）。
    let (lc_out, lc_dur, lc_ttft) = agg.latest_completed.unwrap_or((0, None, None));
    let speed = turn_speed(lc_out, lc_dur, lc_ttft);

    // 模型集合序列化：按最近使用降序（当前模型排首）逗号拼接，注入版
    // models 口径；前端显示首个 + 计数、title 给全量
    let mut models_by_recency: Vec<(String, i64)> = agg.models.into_iter().collect();
    models_by_recency.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    let models_joined = models_by_recency
        .iter()
        .map(|(m, _)| m.as_str())
        .collect::<Vec<_>>()
        .join(",");

    let short = short_session_id(session_id);
    Ok(HudSessionBrief {
        session_id: session_id.to_string(),
        short: short.clone(),
        project: project_name(directory, &short),
        models: if models_joined.is_empty() {
            None
        } else {
            Some(models_joined)
        },
        sub_count: active_subs,
        generating,
        last_active_at,
        in_tokens: agg.plain_in,
        out_tokens: agg.out_tokens,
        cache_read: agg.cache_read,
        total: agg.plain_in + agg.out_tokens + agg.cache_read,
        req_count: agg.req_count,
        speed,
        msg_generating,
        ttft_ms: lc_ttft,
    })
}

/// 会话树聚合中间态（in_tokens 即 ↑ 非缓存输入，Σ 由三分量在出口相加）
#[derive(Default)]
struct SessionAgg {
    plain_in: i64,
    out_tokens: i64,
    cache_read: i64,
    req_count: i64,
    /// 时间窗内模型集合（model_id → 最近使用时刻）
    models: BTreeMap<String, i64>,
    /// 各成员最新一笔（生成中判定按成员取或）
    tails: BTreeMap<String, MemberTail>,
    /// 最近一笔完成请求的 started_at（比较键，初值 MIN 保证首行入选）
    latest_completed_at: i64,
    /// 最近一笔完成请求的 (output, duration_ms, time_to_first_token_ms)
    latest_completed: Option<(i64, Option<i64>, Option<i64>)>,
}

/// 树内单个成员的最新一笔 model_usage 摘要（生成中判定输入）
#[derive(Default)]
struct MemberTail {
    latest_at: i64,
    turn: Option<String>,
    status: Option<String>,
}

/// 动态速度（t/s，口径照抄注入版每轮条，数据源为最近一笔完成请求）：
/// gen = dur − ttft（下限 1ms）；ttft 缺失 → gen = dur；ttft ≥ 90%×dur
///（整块下发）→ gen = ttft；dur 缺失/≤0 或 out ≤ 0 → None（前端显示
/// "–"）。仅在生成中状态调用；空闲由调用方直接归 0.0。
fn turn_speed(out: i64, dur: Option<i64>, ttft: Option<i64>) -> Option<f64> {
    let dur = dur.filter(|d| *d > 0)?;
    if out <= 0 {
        return None;
    }
    let gen = match ttft {
        None => dur,
        Some(t) if t >= dur * 9 / 10 => t,
        Some(t) => (dur - t).max(1),
    };
    Some(out as f64 * 1000.0 / gen.max(1) as f64)
}

/// 数值列降级表达式：列存在取 COALESCE(col, 0)，缺失取常量 0
fn num_expr(has: bool, col: &str) -> String {
    if has {
        format!("COALESCE({col}, 0)")
    } else {
        "0".to_string()
    }
}

/// 可空数值列降级表达式：列存在取原值（可 NULL），缺失取常量 NULL
///（与 usage_feed 的 opt_col 同款语义）
fn opt_expr(has: bool, col: &str) -> String {
    if has {
        col.to_string()
    } else {
        "NULL".to_string()
    }
}

/// 表存在性探测（与 usage_feed 的私有 has_table 同款实现；该函数未从
/// agent_theme 导出且注入体系不可触碰，本模块自持一份）
fn has_table(conn: &Connection, table: &str) -> bool {
    conn.query_row(
        "SELECT COUNT(*) FROM sqlite_master WHERE type = 'table' AND name = ?1",
        [table],
        |row| row.get::<_, i64>(0),
    )
    .map(|c| c > 0)
    .unwrap_or(false)
}

/// turn_usage 中该轮的落库状态：Some("completed"/"error"/"cancelled"/
/// "running") = 已落行；None = 尚未落行（轮进行中）。表/列缺失（老
/// 版本库）一律 None（生成中判定降级为"无未完成轮"）。
fn turn_status(conn: &Connection, session_id: &str, turn_id: &str) -> Result<Option<String>, String> {
    if !has_table(conn, "turn_usage")
        || !crate::db::has_column(conn, "turn_usage", "session_id")
        || !crate::db::has_column(conn, "turn_usage", "turn_id")
        || !crate::db::has_column(conn, "turn_usage", "status")
    {
        return Ok(None);
    }
    match conn.query_row(
        "SELECT status FROM turn_usage WHERE session_id = ?1 AND turn_id = ?2",
        [session_id, turn_id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(status) => Ok(status),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("读取轮状态失败: {e}")),
    }
}

/// 会话短标识：去 "sess_" 前缀后取前 6 位（前缀不剥离的话前 6 位恒为
/// "sess_s" 无区分度）；空/超短 id 原样返回。
fn short_session_id(session_id: &str) -> String {
    let bare = session_id.strip_prefix("sess_").unwrap_or(session_id);
    if bare.is_empty() {
        session_id.chars().take(6).collect()
    } else {
        bare.chars().take(6).collect()
    }
}

/// 项目名解析：directory 取最后一段文件夹名（/ 与 \ 都作分隔符，兼容
/// 跨平台路径）；目录缺失/为空回退 "#短标识"（容错显示，不空白）。
fn project_name(directory: Option<&str>, short: &str) -> String {
    directory
        .map(str::trim)
        .filter(|d| !d.is_empty())
        .and_then(|d| {
            d.split(['/', '\\'])
                .filter(|seg| !seg.is_empty())
                .next_back()
                .map(str::to_string)
        })
        .unwrap_or_else(|| format!("#{short}"))
}

// ============================================================
// rollout 旁路速度源（增量读取 + 滑动窗口）
// ============================================================
//
// 数据事实（2026-09 本机实测）：~/.zcode/cli/rollout/model-io-
// {sessionId}.jsonl（sessionId 即 session 表 id），每行一次模型请求
// 完成即追加，JSON 含 usage.outputTokens（camelCase，注意与 DB 列名
// 不同）与 startedAt（RFC3339 毫秒）。文件由 ZCode 定期清理只留当日
// 活跃会话——HUD 只跟踪活跃会话恰好覆盖，文件缺失静默降级。
// 铁律：rollout 是旁路数据源，只服务速度展示，绝不并入 Σ/↑/↓/⟲/×
// 累计口径（累计以 db.sqlite model_usage 为唯一来源，防双计）。

/// 会话树（主会话 + 子代理）成员清单：DB 轮询产出，驱动 rollout 文件
/// 游标集合（每个成员一个 model-io-{id}.jsonl）。
pub(crate) struct SessionTree {
    /// 主会话 id（行的展示标识）
    pub(crate) root: String,
    /// 树成员（root + 全部子代理）
    pub(crate) members: Vec<String>,
}

/// rollout 单文件游标：偏移续读（参考 zcode_sessions 的 file_progress
/// 模式，仅存内存不落盘——文件由 ZCode 当日清理，HUD 只跟踪活跃会话）。
struct RolloutCursor {
    path: PathBuf,
    offset: u64,
}

/// 速度样本：一次模型请求完成的摊销速率
#[derive(Clone, Copy)]
struct SpeedSample {
    /// 请求开始时刻（rollout startedAt，毫秒）
    start_ms: i64,
    /// 读到该行的本地时刻（毫秒；文件按请求完成追加，近似完成时刻）
    end_ms: i64,
    /// 摊销速率 tokens/s = outputTokens / max(1s, end − start)
    rate: f64,
}

/// 单会话树的速度跟踪器：文件游标 + 样本环形缓冲 + 展示策略状态
///（保持最后非零速度）。
struct TreeSpeed {
    root: String,
    cursors: Vec<RolloutCursor>,
    samples: VecDeque<SpeedSample>,
    /// 最后一次非零窗口速度（"最近生成速率"保持值：生成中窗口归零与
    /// 空闲态都保持显示，新一轮 message 信号开始时重置）
    last_nonzero: Option<f64>,
    /// 是否解析到过任何样本：区分"rollout 全缺失/全解析失败"（回退
    /// DB 最近完成请求口径）与"有数据但窗口已空"（保持最后非零）
    ever_parsed: bool,
    /// message 信号驱动的生成中是否处于活跃（DB 拍同步；false→true
    /// 跳变 = 新一轮开始，触发保持值重置）
    msg_gen_active: bool,
}

impl TreeSpeed {
    /// 新树：每个成员一个游标；新游标从当前文件末尾起读——历史行早于
    /// 滑窗（读了也是陈旧速率），只追增量行才有"实时跳动"意义
    fn new(root: &str, members: &[String], dir: Option<&Path>) -> Self {
        let cursors = members
            .iter()
            .filter_map(|id| {
                let dir = dir?;
                let path = dir.join(format!("model-io-{id}.jsonl"));
                let offset = fs::metadata(&path).map(|m| m.len()).unwrap_or(0);
                Some(RolloutCursor { path, offset })
            })
            .collect();
        Self {
            root: root.to_string(),
            cursors,
            samples: VecDeque::new(),
            last_nonzero: None,
            ever_parsed: false,
            msg_gen_active: false,
        }
    }

    /// 新一轮生成开始（message 信号驱动判定为真）：清除上一轮保持值，
    /// 速度位回到"测算中"态（生成中且窗口无样本 → None → 前端呼吸
    /// 占位），首笔完成请求落库后窗口速度即真实新值
    fn begin_new_round(&mut self) {
        self.last_nonzero = None;
    }

    /// 成员增减后刷新游标集合：已知文件保留偏移续读，新文件从当前
    /// 末尾起读，成员移除的文件游标随之丢弃
    fn refresh_members(&mut self, members: &[String], dir: Option<&Path>) {
        let mut next = Vec::new();
        for id in members {
            let Some(dir) = dir else { break };
            let path = dir.join(format!("model-io-{id}.jsonl"));
            let offset = match self.cursors.iter().find(|c| c.path == path) {
                Some(existing) => existing.offset,
                None => fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
            };
            next.push(RolloutCursor { path, offset });
        }
        self.cursors = next;
    }

    /// 增量读取新完成请求并更新样本缓冲：文件 stat 极轻，size 未变
    /// 跳过；size < offset 视为 truncate/重建归零重读；文件缺失静默
    /// 跳过；只消费完整行，末尾半行留待补完后续读；解析失败的行跳过
    ///（ZCode 私有格式无 schema 承诺）。
    fn ingest(&mut self, now_ms: i64) {
        for cursor in &mut self.cursors {
            let Ok(meta) = fs::metadata(&cursor.path) else {
                continue; // 文件缺失（未运行/已被清理）静默
            };
            let size = meta.len();
            if size == cursor.offset {
                continue; // 无新增
            }
            if size < cursor.offset {
                cursor.offset = 0; // truncate/重建：归零重读
            }
            let Ok(mut file) = fs::File::open(&cursor.path) else {
                continue;
            };
            if file.seek(std::io::SeekFrom::Start(cursor.offset)).is_err() {
                continue;
            }
            let mut buf = Vec::new();
            if file.read_to_end(&mut buf).is_err() {
                continue;
            }
            // 只消费到最后一个完整换行（半行留待下次补完后重读）
            let Some(consumed) = buf.iter().rposition(|&b| b == b'\n') else {
                continue;
            };
            for line in buf[..=consumed].split(|&b| b == b'\n') {
                if line.is_empty() {
                    continue;
                }
                if let Some((start_ms, output)) = parse_rollout_line(line) {
                    self.samples.push_back(SpeedSample {
                        start_ms,
                        end_ms: now_ms,
                        rate: amortized_rate(output, start_ms, now_ms),
                    });
                    self.ever_parsed = true;
                }
            }
            cursor.offset += consumed as u64 + 1;
        }
        // 环形缓冲卫生：滑出保留期的样本丢弃
        self.samples
            .retain(|s| s.end_ms >= now_ms - SAMPLE_RETENTION_MS);
    }

    /// 窗口速度（tokens/s）：各样本与 [now−W, now] 的重叠时长 × 摊销
    /// 速率求和后除以窗口时长——大耗时请求按历史速率摊入窗口，新行
    /// 进入/旧行滑出都会让数字变化，量级正确不虚高
    fn window_speed(&self, now_ms: i64) -> f64 {
        let from = now_ms - SPEED_WINDOW_MS;
        let mut weighted = 0.0f64;
        for s in &self.samples {
            let overlap_end = s.end_ms.min(now_ms);
            let overlap_start = s.start_ms.max(from);
            if overlap_end > overlap_start {
                weighted += (overlap_end - overlap_start) as f64 * s.rate;
            }
        }
        weighted / SPEED_WINDOW_MS as f64
    }

    /// 展示速度决策（口径：速度列 = "该会话最近一次生成的速率"，
    /// 是否正在生成由状态点表达）：
    /// - rollout 全缺失/全解析失败（从未解析到样本）→ DB 最近一笔完成
    ///   请求口径（fallback），生成中与空闲同值不归 0（无测速能力时
    ///   给最近参考值）；
    /// - 生成中：窗口速度实时更新（并存为保持值）；窗口归零（长请求
    ///   进行中无新完成行）→ 保持最后非零；无保持值（新一轮刚开始，
    ///   保持值已随 message 信号重置）→ None（"–"，前端"测算中"呼吸
    ///   占位，不显示上一轮陈旧值）；
    /// - 空闲：冻结保持最后非零（回答完一直显示该轮真实速率，不随
    ///   窗口滑动衰减），无观测历史回退 DB 口径。
    fn display_speed(
        &mut self,
        generating: bool,
        fallback: Option<f64>,
        now_ms: i64,
    ) -> Option<f64> {
        if !self.ever_parsed {
            return fallback;
        }
        let w = self.window_speed(now_ms);
        if generating {
            if w > 0.0 {
                self.last_nonzero = Some(w);
                return Some(w);
            }
            return self.last_nonzero;
        }
        self.last_nonzero.or(fallback)
    }
}

/// rollout 旁路速度监控器（feed 线程私有状态）：按会话树跟踪文件游标
/// 与样本缓冲。
#[derive(Default)]
struct RolloutMonitor {
    trees: Vec<TreeSpeed>,
}

impl RolloutMonitor {
    /// DB 轮询轮同步树集合：消失的树移除，成员增减刷新游标（已知文件
    /// 保留偏移续读，新文件从当前末尾起读）
    fn sync_trees(&mut self, trees: &[SessionTree], dir: Option<&Path>) {
        self.trees
            .retain(|t| trees.iter().any(|nt| nt.root == t.root));
        for nt in trees {
            if self.trees.iter().any(|t| t.root == nt.root) {
                if let Some(existing) = self.trees.iter_mut().find(|t| t.root == nt.root) {
                    existing.refresh_members(&nt.members, dir);
                }
            } else {
                self.trees
                    .push(TreeSpeed::new(&nt.root, &nt.members, dir));
            }
        }
    }

    /// 每拍增量读取（文件 stat 极轻；DB 未变的拍也执行，保证新完成
    /// 请求 1 秒内进入样本）
    fn ingest(&mut self, now_ms: i64) {
        for tree in &mut self.trees {
            tree.ingest(now_ms);
        }
    }

    fn tree_mut(&mut self, root: &str) -> Option<&mut TreeSpeed> {
        self.trees.iter_mut().find(|t| t.root == root)
    }
}

/// rollout 单行解析 → (请求开始毫秒, outputTokens)。ZCode 私有格式
/// 无 schema 承诺：JSON 损坏/字段缺失返回 None 跳过。实测键名
/// camelCase（usage.outputTokens 与 DB 列名不同）。
fn parse_rollout_line(line: &[u8]) -> Option<(i64, i64)> {
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RolloutLine {
        #[serde(default)]
        started_at: Option<String>,
        #[serde(default)]
        response: Option<RolloutResponse>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RolloutResponse {
        #[serde(default)]
        usage: Option<RolloutUsage>,
    }
    #[derive(serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct RolloutUsage {
        #[serde(default)]
        output_tokens: i64,
    }
    let parsed: RolloutLine = serde_json::from_slice(line).ok()?;
    let start_ms = chrono::DateTime::parse_from_rfc3339(parsed.started_at.as_deref()?)
        .ok()?
        .timestamp_millis();
    let output = parsed.response.as_ref()?.usage.as_ref()?.output_tokens;
    Some((start_ms, output))
}

/// 单样本摊销速率（tokens/s）：output ÷ max(1s, end − start)。跨度
/// 不足下限（极快请求/时钟抖动/startedAt 在未来）按 1s 兜底防爆炸。
fn amortized_rate(output: i64, start_ms: i64, end_ms: i64) -> f64 {
    let span = (end_ms - start_ms).max(SAMPLE_MIN_SPAN_MS);
    output as f64 / span as f64 * 1000.0
}

/// rollout 目录（~/.zcode/cli/rollout）：ZBAR_DB 重定向口径与
/// zcode_sessions::cli_root 一致（自定义库时取其上两级）。目录不存在
/// 时仍返回路径（单文件 metadata/open 静默失败即可），None = 无法
/// 定位主目录。
fn rollout_dir() -> Option<PathBuf> {
    let cli = if let Ok(p) = std::env::var("ZBAR_DB") {
        let pb = PathBuf::from(p.trim());
        pb.parent()
            .and_then(|db| db.parent())
            .map(|root| root.to_path_buf())?
    } else {
        dirs::home_dir()?.join(".zcode").join("cli")
    };
    Some(cli.join("rollout"))
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-session-hud-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 会话快照测试库（与 pet.rs 测试同款的 v9 形态 schema，另带
    /// computed_total_tokens / status / session.directory 列）
    fn hud_db(name: &str) -> (Connection, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (
                id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                model_id TEXT, status TEXT,
                input_tokens INTEGER, output_tokens INTEGER, reasoning_tokens INTEGER,
                cache_creation_input_tokens INTEGER, cache_read_input_tokens INTEGER,
                computed_total_tokens INTEGER);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn 配置_默认值读写与camelCase契约() {
        let dir = test_dir("config");
        let path = dir.join("session-hud.json");

        // 默认值写出 → 读回一致
        let default = SessionHudConfig::default();
        assert!(!default.enabled);
        assert_eq!(default.width, HUD_DEFAULT_WIDTH);
        assert_eq!(default.window_minutes, HUD_WINDOW_MINUTES_DEFAULT);
        assert_eq!(default.pos, None);
        assert!(default.show_tokens && default.show_model);
        save_session_hud_config_at(&path, &default).unwrap();
        let back = read_config_at(&path).unwrap();
        assert_eq!(back, default);

        // camelCase 键名（前端契约）
        let text = fs::read_to_string(&path).unwrap();
        for key in [
            "\"enabled\"",
            "\"pos\"",
            "\"width\"",
            "\"opacity\"",
            "\"windowMinutes\"",
            "\"showTokens\"",
            "\"showModel\"",
        ] {
            assert!(text.contains(key), "session-hud.json 缺少字段 {key}：{text}");
        }
        // 旧版配置文件兼容：改版前残留的 showContextBar 字段被 serde
        // 默认忽略，解析不失败（下次保存自然收敛到新字段集）
        fs::write(
            &path,
            r#"{"enabled":true,"showContextBar":false,"windowMinutes":30}"#,
        )
        .unwrap();
        let legacy = read_config_at(&path).unwrap();
        assert!(legacy.enabled);
        assert_eq!(legacy.window_minutes, 30);
        assert_eq!(legacy.show_tokens, true, "缺字段按默认补齐");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 配置_clamp收敛() {
        // 宽度/透明度越界夹回合法域；档位归一到四个合法值
        let c = SessionHudConfig {
            width: 100.0,
            opacity: 5.0,
            window_minutes: 7,
            ..SessionHudConfig::default()
        }
        .clamped();
        assert_eq!(c.width, HUD_WIDTH_RANGE.0);
        assert_eq!(c.opacity, HUD_OPACITY_RANGE.1);
        assert_eq!(c.window_minutes, HUD_WINDOW_MINUTES_DEFAULT);
        // 合法档位原样保留（含"不限"= 0）
        for m in HUD_WINDOW_MINUTES_OPTIONS {
            assert_eq!(
                SessionHudConfig { window_minutes: m, ..SessionHudConfig::default() }
                    .clamped()
                    .window_minutes,
                m
            );
        }
        // NaN 防御回默认宽度
        let c = SessionHudConfig { width: f64::NAN, ..SessionHudConfig::default() }.clamped();
        assert_eq!(c.width, HUD_DEFAULT_WIDTH);
        // 开关不被 clamp 改动
        let c = SessionHudConfig { enabled: true, ..SessionHudConfig::default() }.clamped();
        assert!(c.enabled);
    }

    #[test]
    fn 窗口高度_会话条数自适应与空态下限() {
        // 栅格锁定：与 session-hud.html 的 CSS 常量一一对应
        //（头部 30 / 行 56 / 折叠行 20 / 底部留白 8 / 空态 64）
        assert_eq!(HUD_ROW_H, 56.0);
        // 空列表：空态最小高（显示"暂无活跃会话"，不为 0 高）
        assert_eq!(hud_height(0, false), HUD_EMPTY_HEIGHT);
        assert_eq!(hud_height(0, true), HUD_EMPTY_HEIGHT);
        // 头部 + 行 × 条数 + 底部留白
        assert_eq!(hud_height(1, false), 30.0 + 56.0 + 8.0);
        assert_eq!(hud_height(3, false), 30.0 + 3.0 * 56.0 + 8.0);
        // 超出上限折叠：5 行 + 折叠提示行
        assert_eq!(hud_height(9, true), 30.0 + 5.0 * 56.0 + 20.0 + 8.0);
        // 折叠标志在可见行数内不生效
        assert_eq!(hud_height(2, true), hud_height(2, false) + 0.0);
    }

    #[test]
    fn 项目名_取末段文件夹名与容错() {
        assert_eq!(project_name(Some("/Users/a/proj"), "abc123"), "proj");
        assert_eq!(project_name(Some("/Users/a/proj/"), "abc123"), "proj");
        assert_eq!(project_name(Some("C:\\Users\\a\\proj"), "abc123"), "proj");
        // 目录缺失/空 → "#短标识"（容错显示不空白）
        assert_eq!(project_name(None, "abc123"), "#abc123");
        assert_eq!(project_name(Some("  "), "abc123"), "#abc123");
        assert_eq!(project_name(Some("/"), "abc123"), "#abc123");
    }

    #[test]
    fn 短标识_剥离sess前缀取6位() {
        // "sess_" 前缀不剥离的话前 6 位恒为 "sess_s" 无区分度
        assert_eq!(
            short_session_id("sess_c0ea9749-12df-4917"),
            "c0ea97"
        );
        assert_eq!(short_session_id("sess_ab"), "ab");
        assert_eq!(short_session_id("plain-id-xxx"), "plain-");
        assert_eq!(short_session_id("sess_"), "sess_", "空裸 id 回原串前 6 位");
    }

    #[test]
    fn 快照_活跃窗口聚合与排序折叠() {
        let (conn, path) = hud_db("snapshot");
        let now = 1_000_000_000_i64;
        let win10 = 10;
        let in_window = |ms: i64| now - ms;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_aaa111', NULL, '/Users/a/proj-alpha'),
               ('sess_bbb222', NULL, '/Users/a/proj-beta'),
               ('sess_ccc333', NULL, ''),
               ('sess_sub1', 'sess_aaa111', '/Users/a/proj-alpha');
             INSERT INTO model_usage VALUES
               -- 会话 A：窗口内 1 秒前有请求（最新，应排第一）
               ('sess_aaa111', 'turn_a', {ta}, 'GLM-5.3', 'completed', 100, 200, 0, 10, 50, 300),
               -- 会话 A 早期请求（计入全生命周期累计）
               ('sess_aaa111', 'turn_a0', {ta0}, 'GLM-4.6', 'completed', 5, 6, 0, 0, 0, 11),
               -- 会话 B：窗口内 2 秒前有请求
               ('sess_bbb222', 'turn_b', {tb}, 'GLM-5.3', 'completed', 10, 20, 0, 0, 5, 30),
               -- 子代理会话：窗口内最活跃但不进列表
               ('sess_sub1', 'turn_s', {ts}, 'GLM-5.3', 'completed', 999, 999, 0, 0, 0, 1998);
             INSERT INTO message VALUES
               -- 会话 C：无 model_usage，仅用户消息落库（message 信号覆盖）
               ('msg_c1', 'sess_ccc333', {tc}, '{{\"role\":\"user\"}}');",
            ta = in_window(1_000),
            ta0 = in_window(3_600_000),
            tb = in_window(2_000),
            ts = in_window(500),
            tc = in_window(3_000),
        ))
        .unwrap();

        let snap = collect_session_snapshot(&conn, win10, now).unwrap().0;
        // 子代理会话被过滤；3 个主会话全部活跃
        assert_eq!(snap.total_active, 3, "{snap:?}");
        assert_eq!(snap.v, 1);
        // 排序：按最近活动倒序（A > B > C）
        assert_eq!(snap.sessions.len(), 3);
        assert_eq!(snap.sessions[0].session_id, "sess_aaa111");
        assert_eq!(snap.sessions[1].session_id, "sess_bbb222");
        assert_eq!(snap.sessions[2].session_id, "sess_ccc333");
        // 项目名：directory 末段；空目录回退 "#短标识"
        assert_eq!(snap.sessions[0].project, "proj-alpha");
        assert_eq!(snap.sessions[2].project, "#ccc333");
        // 短标识：剥 sess_ 前缀取 6 位
        assert_eq!(snap.sessions[0].short, "aaa111");
        // 会话树合计（注入版 V9 口径）：子代理 sess_sub1 的窗口内请求
        // 并入主会话 A（含窗口外早期请求），↑ 逐笔 clamp 非缓存口径
        let a = &snap.sessions[0];
        // ↑ = max(0,100−50) + max(0,5−0) + max(0,999−0) = 50+5+999
        assert_eq!(a.in_tokens, 1054);
        // ↓ = (200+6) + 999
        assert_eq!(a.out_tokens, 1205);
        assert_eq!(a.cache_read, 50);
        // Σ = ↑ + ↓ + ⟲（注入版 V15 口径）
        assert_eq!(a.total, 1054 + 1205 + 50);
        // × = 树内 model_usage 行数（每行一笔请求）
        assert_eq!(a.req_count, 3);
        // 子代理徽标：窗口内有活动的子代理数
        assert_eq!(a.sub_count, 1);
        // 模型集合：时间窗内 distinct（GLM-4.6 在窗外不入集），按最近
        // 使用降序拼接
        assert_eq!(a.models.as_deref(), Some("GLM-5.3"));
        // 树内生成中（A 最新轮未落 turn_usage）；最近一笔完成请求的
        // dur/ttft 列在本库缺失 → 速度无数据（前端 "–"）
        assert!(a.generating);
        assert_eq!(a.speed, None);
        assert_eq!(a.ttft_ms, None);
        // 仅 message 信号的会话（C）：user 消息驱动生成中（发消息后
        // 首笔请求完成前的即时反馈），速度 "–"（无数据）
        assert_eq!(snap.sessions[2].models, None);
        assert!(
            snap.sessions[2].generating,
            "user 消息晚于一切请求 → message 驱动生成中"
        );
        assert_eq!(snap.sessions[2].speed, None);
        assert_eq!(snap.sessions[2].ttft_ms, None);
        assert_eq!(snap.sessions[2].total, 0);
        assert_eq!(snap.sessions[2].req_count, 0);

        // 折叠：7 个活跃会话只展示 5 条，total_active = 7
        let (conn, path) = hud_db("fold");
        let mut inserts = String::from(
            "INSERT INTO session VALUES\n",
        );
        let mut usages = String::new();
        for i in 0..7 {
            let id = format!("sess_fold{i:02}");
            inserts.push_str(&format!(
                "('{id}', NULL, '/Users/a/f{i}'),\n"
            ));
            usages.push_str(&format!(
                "INSERT INTO model_usage VALUES ('{id}', 't', {at}, 'M', 'completed', 1, 1, 0, 0, 0, 2);\n",
                at = now - i as i64 * 1_000,
            ));
        }
        inserts.truncate(inserts.len() - 2);
        inserts.push(';');
        conn.execute_batch(&format!("{inserts}\n{usages}")).unwrap();
        let snap = collect_session_snapshot(&conn, win10, now).unwrap().0;
        assert_eq!(snap.total_active, 7);
        assert_eq!(snap.sessions.len(), MAX_VISIBLE_SESSIONS);
        // 窗口档位为"不限"（0）时按 24h 兜底，同样能看到会话
        let snap_unlimited = collect_session_snapshot(&conn, 0, now).unwrap().0;
        assert_eq!(snap_unlimited.total_active, 7);
        // 窗口外会话不可见：档位 5 分钟 = 窗口内全部
        let (conn, path) = hud_db("window");
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_old', NULL, '/o'), ('sess_new', NULL, '/n');
             INSERT INTO model_usage VALUES
               ('sess_old', 't', {old}, 'M', 'completed', 1, 1, 0, 0, 0, 2),
               ('sess_new', 't', {new}, 'M', 'completed', 1, 1, 0, 0, 0, 2);",
            old = now - 20 * 60_000,
            new = now - 60_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 5, now).unwrap().0;
        assert_eq!(snap.total_active, 1, "{snap:?}");
        assert_eq!(snap.sessions[0].session_id, "sess_new");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 快照_生成中判定按最新轮完成状态() {
        let (conn, path) = hud_db("generating");
        let now = 2_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_live', NULL, '/live'), ('sess_done', NULL, '/done'),
               ('sess_fail', NULL, '/fail');
             INSERT INTO model_usage VALUES
               -- 进行中：最新轮无 turn_usage 行 → 生成中
               ('sess_live', 'turn_l1', {l1}, 'M', 'completed', 1, 1, 0, 0, 0, 2);
             INSERT INTO model_usage VALUES
               -- 已完成：turn_usage 有该轮 completed → 空闲
               ('sess_done', 'turn_d1', {d1}, 'M', 'completed', 1, 1, 0, 0, 0, 2);
             INSERT INTO turn_usage VALUES
               ('sess_done', 'turn_d1', 'completed', {d1});
             INSERT INTO model_usage VALUES
               -- 失败：最新请求 error 且轮未落库 → 空闲（防卡生成中）
               ('sess_fail', 'turn_f1', {f1}, 'M', 'error', 1, 0, 0, 0, 0, 1);
             INSERT INTO model_usage VALUES
               -- 轮运行中落库（status='running'）→ 生成中
               ('sess_live2', 'turn_r1', {r1}, 'M', 'completed', 1, 1, 0, 0, 0, 2);",
            l1 = now - 1_000,
            d1 = now - 2_000,
            f1 = now - 3_000,
            r1 = now - 4_000,
        ))
        .unwrap();
        conn.execute_batch(
            "INSERT INTO session VALUES ('sess_live2', NULL, '/live2');
             INSERT INTO turn_usage VALUES ('sess_live2', 'turn_r1', 'running', 0);",
        )
        .unwrap();

        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let gen = |id: &str| {
            snap.sessions
                .iter()
                .find(|s| s.session_id == id)
                .unwrap()
                .generating
        };
        assert!(gen("sess_live"), "最新轮未落 turn_usage 应判生成中");
        assert!(!gen("sess_done"), "完成轮应判空闲");
        assert!(!gen("sess_fail"), "最新请求失败的未落库轮应判空闲");
        assert!(gen("sess_live2"), "turn_usage running 行应判生成中");

        // 老版本库无 status 列：error/cancelled 防卡判定降级为按无轮
        // 行判生成中（与 usage_feed 的 runs 口径一致）。全新裸库自建
        // 缩减 schema（hud_db 会建全量表，不能重复建）
        let path2 = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-nostatus.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path2);
        let conn2 = Connection::open(&path2).unwrap();
        conn2.execute_batch(&format!(
            "CREATE TABLE session (id TEXT PRIMARY KEY);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                input_tokens INTEGER, output_tokens INTEGER);
             INSERT INTO session VALUES ('sess_x');
             INSERT INTO model_usage VALUES ('sess_x', 'turn_x', {t}, 'M', 1, 1);",
            t = now - 1_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn2, 10, now).unwrap().0;
        assert!(snap.sessions[0].generating, "无 status 列时按未完成轮判生成中");

        drop(conn2);
        let _ = fs::remove_file(&path2);
    }

    #[test]
    fn 快照_空库与老版本库降级为空态() {
        // 完全空库（无表）：空快照不报错
        let path = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-empty.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        let snap = collect_session_snapshot(&conn, 10, 1_000).unwrap().0;
        assert!(snap.sessions.is_empty());
        assert_eq!(snap.total_active, 0);
        drop(conn);
        let _ = fs::remove_file(&path);

        // 有 session 表但无 model_usage 表（老版本库）：空态
        let (conn, path) = hud_db("old");
        conn.execute_batch("DROP TABLE model_usage;").unwrap();
        let snap = collect_session_snapshot(&conn, 10, 1_000).unwrap().0;
        assert!(snap.sessions.is_empty());
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 口径_非缓存输入clamp与Σ合成() {
        // 个别行 cache_read 大于 input（异常/口径交叉）：↑ 按行 clamp 0，
        // 注入版"保守取小值"同款；Σ = ↑ + ↓ + ⟲
        let (conn, path) = hud_db("clamp");
        let now = 5_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_clamp', NULL, '/c');
             INSERT INTO model_usage VALUES
               -- input 30 < cache_read 50 → 该行 ↑ 取 0
               ('sess_clamp', 't1', {t1}, 'M', 'completed', 30, 7, 0, 0, 50, 37),
               -- 正常行：input 100, cache_read 40 → ↑ = 60
               ('sess_clamp', 't2', {t2}, 'M', 'completed', 100, 3, 0, 0, 40, 103);",
            t1 = now - 2_000,
            t2 = now - 1_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let s = &snap.sessions[0];
        assert_eq!(s.in_tokens, 0 + 60, "↑ 应逐行 clamp 负值后汇总");
        assert_eq!(s.out_tokens, 10);
        assert_eq!(s.cache_read, 90);
        assert_eq!(s.total, 60 + 10 + 90, "Σ 应为 ↑+↓+⟲（注入版 V15 口径）");
        assert_eq!(s.req_count, 2, "× = model_usage 行数");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度_gen三分支与降级() {
        // 常规：gen = dur − ttft（下限 1ms）→ 100 × 1000 / 4000 = 25.0
        let s = turn_speed(100, Some(5_000), Some(1_000)).unwrap();
        assert!((s - 25.0).abs() < 1e-9, "{s}");
        // ttft 缺失：gen = dur → 100 × 1000 / 5000 = 20.0
        let s = turn_speed(100, Some(5_000), None).unwrap();
        assert!((s - 20.0).abs() < 1e-9, "{s}");
        // ttft ≥ 90%×dur（整块下发）：gen = ttft → 100 × 1000 / 4800
        let s = turn_speed(100, Some(5_000), Some(4_800)).unwrap();
        assert!((s - 100.0 * 1000.0 / 4_800.0).abs() < 1e-9, "{s}");
        // 边界：ttft 恰为 90%×dur → 走整块下发分支
        let s = turn_speed(100, Some(5_000), Some(4_500)).unwrap();
        assert!((s - 100.0 * 1000.0 / 4_500.0).abs() < 1e-9, "{s}");
        // dur 缺失/≤0、out ≤ 0 → None（前端显示 "–"）
        assert_eq!(turn_speed(100, None, Some(1_000)), None);
        assert_eq!(turn_speed(100, Some(0), None), None);
        assert_eq!(turn_speed(100, Some(-5), None), None);
        assert_eq!(turn_speed(0, Some(5_000), None), None);
        assert_eq!(turn_speed(-1, Some(5_000), None), None);
    }

    #[test]
    fn 子代理归并_主空闲子活跃仍显示且合计() {
        let (conn, path) = hud_db("merge");
        let now = 7_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_m', NULL, '/merge'), ('sess_s1', 'sess_m', '/merge'),
               ('sess_s2', 'sess_m', '/merge');
             INSERT INTO model_usage VALUES
               -- 主会话自身 20 分钟前（10 分钟窗口外，自身已空闲）
               ('sess_m', 'turn_old', {old}, 'GLM-4.6', 'completed', 10, 10, 0, 0, 0, 20),
               -- 两个子代理窗口内活跃
               ('sess_s1', 'turn_sa', {s1}, 'GLM-5.3', 'completed', 100, 200, 0, 0, 0, 300),
               ('sess_s2', 'turn_sb', {s2}, 'GLM-5.3', 'completed', 50, 60, 0, 0, 0, 110);",
            old = now - 20 * 60_000,
            s1 = now - 1_000,
            s2 = now - 2_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        // 主会话自身窗口外空闲，但子代理活跃 → 主会话行仍显示且唯一
        assert_eq!(snap.total_active, 1, "{snap:?}");
        assert_eq!(snap.sessions.len(), 1);
        let m = &snap.sessions[0];
        assert_eq!(m.session_id, "sess_m");
        // last_active 归并取子代理最大值（排序键不被主会话空闲拖没）
        assert_eq!(m.last_active_at, now - 1_000);
        // 树合计：↑ = 10 + 100 + 50；↓ = 10 + 200 + 60；⟲ = 0
        assert_eq!(m.in_tokens, 160);
        assert_eq!(m.out_tokens, 270);
        assert_eq!(m.cache_read, 0);
        assert_eq!(m.total, 160 + 270);
        assert_eq!(m.req_count, 3);
        // 子代理徽标：窗口内有活动的子代理数
        assert_eq!(m.sub_count, 2);
        // 模型集合：窗口内只有 GLM-5.3（主会话的 GLM-4.6 在窗外不入集）
        assert_eq!(m.models.as_deref(), Some("GLM-5.3"));
        // 生成中：主会话最新轮（turn_old）无 turn_usage 落库 → 树内生成中
        assert!(m.generating);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 模型集合_窗口内去重最新优先与子代理并入() {
        let (conn, path) = hud_db("models");
        let now = 8_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_m', NULL, '/m'), ('sess_s1', 'sess_m', '/m');
             INSERT INTO model_usage VALUES
               ('sess_m', 't1', {t1}, 'GLM-4.6', 'completed', 10, 10, 0, 0, 0, 20),
               ('sess_m', 't2', {t2}, 'GLM-5.3', 'completed', 10, 10, 0, 0, 0, 20),
               -- 窗口外的旧模型不入集合
               ('sess_m', 't0', {t0}, 'GLM-3', 'completed', 10, 10, 0, 0, 0, 20),
               -- 子代理窗口内的模型并入集合
               ('sess_s1', 't9', {t9}, 'GLM-5.3-Flash', 'completed', 10, 10, 0, 0, 0, 20);",
            t1 = now - 60_000,
            t2 = now - 5_000,
            t0 = now - 2 * 3_600_000,
            t9 = now - 10_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let m = &snap.sessions[0];
        // 按最近使用降序：GLM-5.3(5s) → GLM-5.3-Flash(10s) → GLM-4.6(60s)
        assert_eq!(
            m.models.as_deref(),
            Some("GLM-5.3,GLM-5.3-Flash,GLM-4.6")
        );
        // 子代理徽标：S1 窗口内有活动
        assert_eq!(m.sub_count, 1);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度_最近完成请求降级路径与树内取或() {
        let now = 9_000_000_000_i64;
        // 完整列 schema：model_usage 带 duration_ms / time_to_first_token_ms
        let path = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-speed.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE session (id TEXT PRIMARY KEY, parent_id TEXT, directory TEXT);
             CREATE TABLE turn_usage (
                session_id TEXT, turn_id TEXT, status TEXT, started_at INTEGER);
             CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, duration_ms INTEGER,
                time_to_first_token_ms INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_m', NULL, '/m'), ('sess_s1', 'sess_m', '/m');
             INSERT INTO model_usage VALUES
               -- 主会话较早完成请求（常规 gen 分支数据，非数据源）
               ('sess_m', 'tm1', {t1}, 'GLM-5.3', 'completed', 100, 200, 0, 5_000, 1_000),
               -- 子代理完成请求
               ('sess_s1', 'ts1', {t2}, 'GLM-5.3', 'completed', 50, 100, 0, 4_000, 500),
               -- 主会话最新完成请求（应作为速度数据源：ttft 7500 ≥ 90%×8000
               -- → gen = 7500 → 300 × 1000 / 7500 = 40.0）
               ('sess_m', 'tm2', {t3}, 'GLM-4.7', 'completed', 10, 300, 0, 8_000, 7_500);",
            t1 = now - 30_000,
            t2 = now - 20_000,
            t3 = now - 10_000,
        ))
        .unwrap();
        // 主会话最新轮已落完成行 → 自身空闲
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES ('sess_m', 'tm2', 'completed', {t});",
            t = now - 10_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let m = &snap.sessions[0];
        // 树内取或：子代理最新轮（ts1）无 turn_usage 落库 → 生成中
        assert!(m.generating, "{m:?}");
        // 生成中：速度取树内最近一笔完成请求（主会话 tm2，started_at 最新）
        let speed = m.speed.expect("生成中应有速度值");
        assert!((speed - 40.0).abs() < 1e-9, "{speed}");
        assert_eq!(m.ttft_ms, Some(7_500));

        // 子代理轮落完成行 → 全树空闲 → "最近生成速率"语义：DB fallback
        // 空闲保持最近一笔完成请求速度不归 0，TTFT 保持最近一笔参考值
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES ('sess_s1', 'ts1', 'completed', {t});",
            t = now - 20_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let m = &snap.sessions[0];
        assert!(!m.generating);
        let idle_speed = m.speed.expect("空闲应保持最近生成速率（DB fallback）");
        assert!((idle_speed - 40.0).abs() < 1e-9, "{idle_speed}");
        assert_eq!(m.ttft_ms, Some(7_500));
        drop(conn);
        let _ = fs::remove_file(&path);

        // 缩减列 schema（hud_db 无 dur/ttft 列）：生成中且有完成请求，
        // 但数据不足 → 速度 None（前端 "–"）、TTFT None
        let (conn, path) = hud_db("speed-legacy");
        let now2 = 9_500_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_leg', NULL, '/l');
             INSERT INTO model_usage VALUES
               ('sess_leg', 'tl', {t}, 'M', 'completed', 10, 500, 0, 0, 0, 510);",
            t = now2 - 1_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now2).unwrap().0;
        let m = &snap.sessions[0];
        assert!(m.generating, "最新轮未落 turn_usage → 生成中");
        assert_eq!(m.speed, None, "dur/ttft 列缺失 → 速度无数据");
        assert_eq!(m.ttft_ms, None);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// rollout 单行测试 JSON（camelCase 键名与实测格式一致，结尾换行）
    fn line_json(start_ms: i64, output: i64) -> String {
        let ts = chrono::DateTime::from_timestamp_millis(start_ms)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        format!(
            "{{\"sessionId\":\"sess_x\",\"startedAt\":\"{ts}\",\"response\":{{\"usage\":{{\"outputTokens\":{output}}}}}}}\n"
        )
    }

    /// 速度样本快捷构造
    fn sample(start_ms: i64, end_ms: i64, rate: f64) -> SpeedSample {
        SpeedSample {
            start_ms,
            end_ms,
            rate,
        }
    }

    #[test]
    fn 速度_样本摊销速率与窗口重叠求速() {
        let now = 10_000_000_000_i64;
        // 摊销速率：10s 内完成 1000 tokens → 100 t/s
        assert!((amortized_rate(1_000, now - 10_000, now) - 100.0).abs() < 1e-9);
        // 跨度不足 1s（极快请求）按 1s 兜底 → 1000 t/s
        assert!((amortized_rate(1_000, now - 200, now) - 1_000.0).abs() < 1e-9);
        // startedAt 在未来（时钟抖动）→ 同样兜底 1s
        assert!((amortized_rate(500, now + 5_000, now) - 500.0).abs() < 1e-9);

        // 窗口重叠求速（W=4s）：重叠时长 × 速率 求和 / 4s
        let mut tree = TreeSpeed::new("t", &["t".into()], None);
        // 旧请求横跨窗口起点（now−4s）：重叠 [now−4s, now−2s] = 2s × 10 = 20
        tree.samples.push_back(sample(now - 6_000, now - 2_000, 10.0));
        // 窗口内短请求：重叠 1s × 50 = 50
        tree.samples.push_back(sample(now - 2_000, now - 1_000, 50.0));
        // 完全在窗口外（end 早于 now−4s）→ 不贡献
        tree.samples
            .push_back(sample(now - 30_000, now - 20_000, 999.0));
        // end 在未来（时钟抖动）按 now 截断：重叠 1s × 8 = 8
        tree.samples.push_back(sample(now - 1_000, now + 5_000, 8.0));
        let w = tree.window_speed(now);
        let w_sec = SPEED_WINDOW_MS as f64 / 1000.0;
        assert!((w - (20.0 + 50.0 + 8.0) / w_sec).abs() < 1e-9, "{w}");
    }

    #[test]
    fn 速度_truncate重置与半行续读() {
        let dir = test_dir("rollout");
        let path = dir.join("model-io-t.jsonl");
        let now = 11_000_000_000_i64;
        let mut tree = TreeSpeed::new("t", &["t".into()], Some(dir.as_path()));
        tree.cursors = vec![RolloutCursor {
            path: path.clone(),
            offset: 0,
        }];

        // 写入一行完整 JSON → 解析出 1 个样本，offset 推进到行尾
        std::fs::write(&path, line_json(now - 1_000, 100)).unwrap();
        tree.ingest(now);
        assert_eq!(tree.samples.len(), 1);
        assert_eq!(tree.cursors[0].offset, path.metadata().unwrap().len());
        assert!(tree.ever_parsed);

        // 追加半行（无换行结尾）→ 不消费，offset 停在完整行末尾
        let full = line_json(now - 500, 100);
        let partial = &full[..full.len() - 1]; // 掐掉结尾换行制造半行
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        use std::io::Write as _;
        file.write_all(partial.as_bytes()).unwrap();
        drop(file);
        tree.ingest(now);
        assert_eq!(tree.samples.len(), 1, "半行不应解析");
        assert_eq!(
            tree.cursors[0].offset,
            path.metadata().unwrap().len() - partial.len() as u64
        );

        // 半行补上换行 → 完整行被解析出第 2 个样本
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n").unwrap();
        drop(file);
        tree.ingest(now);
        assert_eq!(tree.samples.len(), 2);
        assert_eq!(tree.cursors[0].offset, path.metadata().unwrap().len());

        // truncate/重建：文件变小 → offset 归零重读，不报错
        std::fs::write(&path, line_json(now - 200, 700)).unwrap();
        assert!(path.metadata().unwrap().len() < tree.cursors[0].offset);
        tree.ingest(now);
        assert_eq!(tree.cursors[0].offset, path.metadata().unwrap().len());
        // 样本缓冲含 truncate 后新行的摊销速率（700 tokens / 1s 兜底）
        assert!(tree.samples.iter().any(|s| (s.rate - 700.0).abs() < 1e-9));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 速度_展示策略_最近生成速率语义() {
        let now = 12_000_000_000_i64;
        let mut tree = TreeSpeed::new("t", &["t".into()], None);

        // rollout 全缺失（从未解析到样本）→ DB 最近完成请求口径，
        // 生成中与空闲同值不归 0（"最近生成速率"语义）
        assert_eq!(tree.display_speed(true, Some(7.0), now), Some(7.0));
        assert_eq!(tree.display_speed(false, Some(7.0), now), Some(7.0));
        assert_eq!(tree.display_speed(false, None, now), None);

        // 生成中、有解析史但窗口无样本且无保持值（新一轮刚开始，保持
        // 值已随 message 信号重置）→ None（"–"，前端"测算中"呼吸占位，
        // 不回退 DB 旧速度显示上一轮陈旧值）
        tree.ever_parsed = true;
        assert_eq!(tree.display_speed(true, Some(7.0), now), None);

        // 生成中、窗口有新行 → 窗口速度（并存为保持值）
        tree.samples.push_back(sample(now - 1_000, now, 25.0));
        let s = tree.display_speed(true, Some(7.0), now);
        assert!(s.unwrap() > 0.0);
        let kept = tree.last_nonzero;
        assert!(kept.unwrap() > 0.0);

        // 生成中、样本滑出窗口归零 → 保持最后非零（不闪 0）
        tree.samples.clear();
        assert_eq!(tree.display_speed(true, Some(7.0), now + 10_000), kept);

        // 空闲 → 冻结保持最后非零（回答完一直显示该轮真实速率，不归 0
        // 不随窗口滑动衰减），无保持值时回退 DB 口径
        assert_eq!(tree.display_speed(false, Some(7.0), now + 20_000), kept);
        tree.last_nonzero = None;
        assert_eq!(tree.display_speed(false, Some(7.0), now + 20_000), Some(7.0));

        // 新一轮重置：清除保持值 → 生成中无样本回到"测算中"（None）
        tree.begin_new_round();
        assert_eq!(tree.display_speed(true, Some(7.0), now + 30_000), None);
    }

    #[test]
    fn 生成中_message信号驱动与树口径() {
        let (conn, path) = hud_db("msg-gen");
        let now = 13_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_m', NULL, '/m'), ('sess_s1', 'sess_m', '/m');
             -- 主会话：一笔已完成请求 + 轮已落库（按原轮级判定为空闲）
             INSERT INTO model_usage VALUES
               ('sess_m', 'tm1', {t1}, 'M', 'completed', 10, 10, 0, 0, 0, 20);
             INSERT INTO turn_usage VALUES
               ('sess_m', 'tm1', 'completed', {t1});
             -- 主会话一条助手消息（比请求晚，不得驱动生成中）
             INSERT INTO message VALUES
               ('msg_a', 'sess_m', {ta}, '{{\"role\":\"assistant\"}}');
             -- 子代理会话一条 user 消息（比请求晚 → 树口径驱动生成中）
             INSERT INTO message VALUES
               ('msg_u', 'sess_s1', {tu}, '{{\"role\":\"user\"}}');",
            t1 = now - 60_000,
            ta = now - 5_000,
            tu = now - 2_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let m = &snap.sessions[0];
        // user 消息（子代理成员，树口径）晚于树内最新请求开始 → 生成中
        //（新轮已开始、首笔请求未完成，"–" 语义的 speed 数据不足）
        assert!(m.generating, "{m:?}");
        assert_eq!(m.speed, None);

        // user 消息早于最新请求开始（首笔已完成）且轮已落库 → 按原判定
        //（空闲）；此时新增的助手消息不得把刚完成的轮误判回生成中
        //（先移除子代理的新消息，树内只留更早的 user 消息与助手消息）
        conn.execute_batch(&format!(
            "DELETE FROM message WHERE id = 'msg_u';
             INSERT INTO message VALUES
               ('msg_u0', 'sess_m', {t0}, '{{\"role\":\"user\"}}');",
            t0 = now - 70_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 10, now).unwrap().0;
        assert!(
            !snap.sessions[0].generating,
            "user 消息早于最新请求开始且轮完成 → 空闲"
        );

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// 实跑冒烟：依赖本机 ~/.zcode/cli/rollout 真实文件（显式
    /// `cargo test --lib session_hud -- --ignored --nocapture` 运行）
    #[test]
    #[ignore = "实跑冒烟：依赖本机 ~/.zcode/cli/rollout 真实文件"]
    fn 实跑_rollout增量读与窗口速度冒烟() {
        let Some(dir) = rollout_dir() else {
            eprintln!("无法定位 rollout 目录");
            return;
        };
        let mut files: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map(|rd| {
                rd.flatten()
                    .map(|e| e.path())
                    .filter(|p| {
                        p.file_name()
                            .and_then(|n| n.to_str())
                            .map(|n| {
                                n.starts_with("model-io-") && n.ends_with(".jsonl")
                            })
                            .unwrap_or(false)
                    })
                    .collect()
            })
            .unwrap_or_default();
        files.sort();
        if files.is_empty() {
            eprintln!("rollout 目录无 model-io 文件：{}", dir.display());
            return;
        }
        // 取最近修改的文件全量读（offset 0），验证真实格式解析
        let newest = files.last().unwrap().clone();
        let now = chrono::Utc::now().timestamp_millis();
        let mut tree = TreeSpeed::new("smoke", &["smoke".into()], Some(dir.as_path()));
        tree.cursors = vec![RolloutCursor {
            path: newest.clone(),
            offset: 0,
        }];
        tree.ingest(now);
        // 窗口速度按真实"最近 4 秒"计算：本会话恰在活跃生成则非零
        let w = tree.window_speed(now);
        tree.ever_parsed = true;
        let display = tree.display_speed(true, Some(0.0), now);
        eprintln!(
            "文件 {} 解析样本 {} 条（保留期内），{}s 窗口速度 {:.1} t/s，展示值 {:?}",
            newest.display(),
            tree.samples.len(),
            SPEED_WINDOW_MS / 1000,
            w,
            display
        );
        assert!(tree.ever_parsed, "最新 rollout 文件应至少解析出一行");
    }

    // ---- 文件路径显式版读写（测试复用，与生产 save/load 同逻辑）----

    fn save_session_hud_config_at(path: &std::path::Path, config: &SessionHudConfig) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&config.clone().clamped())
            .map_err(|e| format!("序列化会话悬浮窗配置失败: {e}"))?;
        let tmp = path.with_extension("json.tmp");
        fs::write(&tmp, json).map_err(|e| format!("写入会话悬浮窗配置失败: {e}"))?;
        fs::rename(&tmp, path).map_err(|e| format!("保存会话悬浮窗配置失败: {e}"))
    }

    fn read_config_at(path: &std::path::Path) -> Option<SessionHudConfig> {
        let text = fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }
}

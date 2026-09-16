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
//! 速度：DB 的请求粒度足以提供最近一笔已完成请求的速度，速度走共享
//! 请求级计算规则；rollout 文件只在 DB 尚未可见时提供旁路参考。每笔请求
//! 完成后追加一行，偏移续读增量解析；有首字与完成时刻时显示可信生成速度，
//! 只有请求总耗时则显示带 `≈` 质量标记的请求平均速度。缺失/逆序/未来
//! 时间不生成速度，也不沿用旧值。速度节拍只负责发现新请求，不让已有请求
//! 随时间衰减。rollout 只服务速度展示，绝不并入 Σ/↑/↓/⟲/× 累计口径
//!（累计以 db.sqlite model_usage 为唯一来源，防双计）。
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
use std::collections::{BTreeMap, BTreeSet};
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

use crate::token_speed::{
    is_completed_status, request_speed_at, PENDING_FRESH_MS, RequestTiming, SessionTree,
    SpeedQuality, SpeedSnapshot, SpeedState,
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

/// 悬浮窗默认宽度（逻辑 px，基准尺寸；用户从未拖拽窗口大小时使用，
/// 高度随会话条数自适应。用户拖拽后宽度/高度成对持久化，见
/// SessionHudConfig.height）
pub const HUD_DEFAULT_WIDTH: f64 = 300.0;
/// 自由拖拽宽高的最小值（逻辑 px，与建窗 min_inner_size 一致；小于该
/// 尺寸的 Resized 事件（最小化/系统抖动）不落盘）
pub const HUD_MIN_WIDTH: f64 = 260.0;
pub const HUD_MIN_HEIGHT: f64 = 160.0;
/// 拖拽尺寸上限（逻辑 px，防脏值/极端最大化落盘；正常屏幕远小于此）
const HUD_MAX_WIDTH: f64 = 4096.0;
const HUD_MAX_HEIGHT: f64 = 4096.0;
/// 字体缩放合法域与默认值（悬浮窗设置面板字体滑块 0.8~1.4 步进 0.05；
/// 脏值回退默认）
pub const HUD_FONT_SCALE_RANGE: (f64, f64) = (0.8, 1.4);
pub const HUD_FONT_SCALE_DEFAULT: f64 = 1.0;
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
/// 模型速度区扫查的行数上限（活跃窗口内最近 N 笔请求，started_at 降序
/// LIMIT）：窗口（尤其"不限"档的 24h）内行数可能很大，以最近 N 笔为界
/// 保证查询成本有硬上限；极重负载下更早的完成请求不参与速度聚合属可
/// 接受窄边缘（展示只取每模型最近值 + 窗口均值，最近样本必然在内）
const MODEL_SPEED_SCAN_ROWS: i64 = 2000;
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

/// 窗口布局常量（逻辑 px，font_scale = 1 基准）：与 session-hud.html /
/// session-hud-main.ts 的 CSS 尺寸一一对应（头部拖动区 / 会话行 / 模型
/// 速度行 / 折叠提示行 / 底部留白），Rust 侧据此按会话条数计算窗口高
/// 度。行高 56 = 三行制内容（项目行 14 + 核心指标行 14 + 分解行 14 +
/// 行距 3×2）+ 上下内边距 4×2。CSS 侧字号/行高/这些行高全部随
/// --hud-scale（fontScale）calc 缩放，本公式在出口整体乘 font_scale
/// 保持与内容栅格一致。
const HUD_HEADER_H: f64 = 30.0;
const HUD_ROW_H: f64 = 56.0;
const HUD_MORE_H: f64 = 20.0;
/// 今日合计行高（逻辑 px，与折叠行同高）：列表下方、折叠行之上的独立
/// 汇总行（"今日 Σ x · × n"，窗口级汇总随 show_tokens 配置隐藏；关闭
/// 数据行显示、今日无请求或会话列表为空时不显示）
const HUD_TODAY_H: f64 = 20.0;
/// 模型速度区单行高（逻辑 px，与今日行同高）：列表与今日行之间的按模型
/// 分组速度行（每活跃模型一行，左模型名右速度），随 show_tokens 配置
/// 隐藏、空样本或空态不显示
const HUD_MODEL_ROW_H: f64 = 20.0;
/// 模型速度区最多行数（窗口内最近活跃的前 3 个模型，超出不展示）
const HUD_MAX_MODEL_ROWS: usize = 3;
/// 列表区容器 border-bottom（逻辑 px）：#hud-list:not(:empty) 有一条
/// 1px 下边框（兼作与模型速度区/今日行的分隔线），随可见行一起出现，
/// 高度公式必须计入（否则窗口比内容矮 1px，末行被裁）
const HUD_LIST_BORDER_H: f64 = 1.0;
/// 模型速度区容器 border-bottom（逻辑 px）：#hud-models 的 1px 下边框
///（与今日行的分隔线或自身底部收边），区可见时计入
const HUD_MODELS_BORDER_H: f64 = 1.0;
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

/// 会话悬浮窗配置（皮肤页"会话悬浮窗"卡读写 + 窗口自由拖拽尺寸/字体
/// 缩放持久化）。serde camelCase 与前端契约字段一一对应；
/// `#[serde(default)]` 旧版文件缺字段按默认补齐，未知名（如改版前残留
/// 的 showContextBar）serde 默认忽略，旧配置文件解析不受影响，下次保存
/// 自然收敛到新字段集。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct SessionHudConfig {
    /// 总开关：false = 关窗停轮询；true = 建窗 + 启轮询
    pub enabled: bool,
    /// 窗口左上角位置（逻辑坐标 x/y，拖动结束落盘，重启恢复）；
    /// None = 从未拖动过，创建时默认主显示器右下角（宠物窗上方）
    pub pos: Option<(f64, f64)>,
    /// 窗口宽度（逻辑 px）。语义（V3 起）：宽度滑块已移除，本字段转为
    /// "用户拖拽尺寸持久化"用途——从未拖拽时为默认宽度；拖拽后与 height
    /// 成对落盘，重启按持久化尺寸恢复
    pub width: f64,
    /// 窗口高度（逻辑 px）：Some = 用户拖拽过窗口（自由尺寸模式，内容
    /// 超出高度时会话列表区纵向滚动，模型速度区/今日行固定不滚）；
    /// None = 从未拖拽（自适应模式，高度按 hud_height 公式随会话条数
    /// 自适应）。恢复自适应方式：配置文件把 height 手改为 null（无 UI
    /// 入口，属进阶操作）
    pub height: Option<f64>,
    /// 字体缩放系数（0.8~1.4，悬浮窗设置面板字体滑块调节）：页面经 CSS
    /// 变量 --hud-scale 对全部字号/行高/行高栅格 calc 缩放，自适应高度按
    /// hud_height × font_scale 同步缩放保持内容不被裁；脏值回退 1.0
    pub font_scale: f64,
    /// 窗口不透明度（0.25~1.0，悬浮窗设置面板透明度滑块调节，前端经
    /// CSS opacity 应用到悬浮窗根节点；底色 token 不透明，见
    /// session-hud.html 的 V4 修复说明）
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
            height: None,
            font_scale: HUD_FONT_SCALE_DEFAULT,
            opacity: HUD_OPACITY_DEFAULT,
            window_minutes: HUD_WINDOW_MINUTES_DEFAULT,
            show_tokens: true,
            show_model: true,
        }
    }
}

impl SessionHudConfig {
    /// 把越界参数收敛到合法范围（保存前的防御，脏数据不落盘）：宽度/
    /// 高度夹回拖拽合法域、透明度夹回合法域、档位归一到四个合法值
    ///（其余脏值一律回默认 10 分钟）、字体缩放脏值回退 1.0
    pub fn clamped(mut self) -> Self {
        if !self.width.is_finite() {
            self.width = HUD_DEFAULT_WIDTH;
        }
        self.width = self.width.clamp(HUD_MIN_WIDTH, HUD_MAX_WIDTH);
        self.height = match self.height {
            Some(h) if h.is_finite() => Some(h.clamp(HUD_MIN_HEIGHT, HUD_MAX_HEIGHT)),
            _ => None,
        };
        if !self.font_scale.is_finite()
            || !(HUD_FONT_SCALE_RANGE.0..=HUD_FONT_SCALE_RANGE.1).contains(&self.font_scale)
        {
            self.font_scale = HUD_FONT_SCALE_DEFAULT;
        }
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

/// 局部更新配置的纯函数内核（悬浮窗内两个滑块命令共用，供单元测试直接
/// 驱动）：只动目标字段，整体 clamp 收敛后返回——未涉及的字段（窗口
/// 尺寸/位置/档位/开关等）原样保留，脏值不落盘。
fn apply_partial_update(
    cfg: SessionHudConfig,
    update: impl FnOnce(&mut SessionHudConfig),
) -> SessionHudConfig {
    let mut next = cfg;
    update(&mut next);
    next.clamped()
}

/// 局部更新落盘的公共管道（字体缩放 / 透明度两个悬浮窗内滑块共用）：
/// 读盘 → 更新单个字段并 clamp → 落盘 → 热推参数。不走
/// set_session_hud_config 的建/关窗流程（滑块在悬浮窗内，用户拖动的瞬间
/// 窗口一定存在，无需窗口操作；也避免整份配置回传竞态——面板侧可能正持有
/// 旧快照）。窗口不存在时仅落盘（下次开窗生效）。
fn save_partial_update(
    app: &AppHandle,
    update: impl FnOnce(&mut SessionHudConfig),
) -> Result<SessionHudConfig, String> {
    let cfg = apply_partial_update(load_session_hud_config(), update);
    save_session_hud_config(&cfg)?;
    push_session_hud_params(app, &cfg);
    Ok(cfg)
}

/// 更新字体缩放（悬浮窗设置面板字体滑块专用轻量命令）：只改 fontScale
/// 一个字段并落盘 + 热推参数。返回 clamp 后的完整配置供调用方回读校准。
#[tauri::command]
pub fn set_session_hud_font_scale(scale: f64, app: AppHandle) -> Result<SessionHudConfig, String> {
    save_partial_update(&app, |cfg| cfg.font_scale = scale)
}

/// 更新窗口透明度（悬浮窗设置面板透明度滑块专用轻量命令）：只改 opacity
/// 一个字段（刻度 / 100，clamp 到 0.25~1.0）并落盘 + 热推参数。返回
/// clamp 后的完整配置供调用方回读校准（热推回环带回同一值，页面本地
/// 已应用的值不会抖动）。
#[tauri::command]
pub fn set_session_hud_opacity(opacity: f64, app: AppHandle) -> Result<SessionHudConfig, String> {
    save_partial_update(&app, |cfg| cfg.opacity = opacity)
}

/// 关闭悬浮窗（悬浮窗内"×"按钮）：总开关置 false 后走与设置页总开关
/// 关闭完全同一条生效路径（停轮询 + 关窗），其余字段（位置/尺寸/透明度/
/// 档位/显示项）原样保留，用户再次从设置页开启时恢复原样。窗口随后销毁，
/// 前端不处理返回（Destroyed 挂点读到的开关已关，幂等不改写）。
#[tauri::command]
pub async fn close_session_hud(app: AppHandle) -> Result<SessionHudConfig, String> {
    let mut cfg = load_session_hud_config().clamped();
    cfg.enabled = false;
    apply_session_hud_config(app, cfg).await
}

/// 用户拖拽热区调整尺寸：会话开始（悬浮窗热区 pointerdown 调用）。置位
/// "用户调整中"原子标志（poll_db 在其期间暂停程序侧 set_size，见
/// HUD_USER_RESIZING 的防打架说明），并读取当前窗口实际尺寸存为拖前快照
///（HUD_RESIZE_SNAPSHOT）：用户纯拖宽度（West 向）时 Resized 事件的高度
/// 仍是拖前自适应值，persist_hud_size 据快照识别"高度轴没动"从而不把
/// 自适应高度固化成用户固定高度（方向感知，见 merge_persist_axes）。快照
/// 读取失败（窗口不存在 / 主线程异常）保持 None = 放弃方向感知，persist
/// 退化为现状两轴都写。async 命令 + run_on_main_thread 读取（同步命令占
/// 主线程，直接调窗口 API 会自等待死锁，见 set_session_hud_config 的同款
/// 死锁防护说明）。
#[tauri::command]
pub async fn session_hud_resize_begin(app: AppHandle) {
    set_user_resizing(true);
    // 先清旧快照再读取：上次会话的残留快照会污染本次高度轴判定
    clear_resize_snapshot();
    // 先置标志后读快照：读取期间 poll_db 的 set_size 已被标志拦截（含已
    // 投递闭包的 should_apply_size_now 双检），读到的是干净的拖前值。注：
    // 快照读取是异步的，用户飞快拖拽时可能晚于首个 Resized 落盘——该窗口
    // 内 persist 退化为现状行为，属可接受窄边缘（见 HUD_RESIZE_SNAPSHOT）
    let snapshot = read_window_logical_size(&app).ok().flatten();
    if let Some(s) = snapshot {
        store_resize_snapshot(s);
    }
}

/// 用户拖拽热区调整尺寸：会话结束（pointerup / pointercancel 调用）。
/// **先完成终值读取与落盘，最后才清标志**——顺序是关键（Bug 修复）：若先
/// 清标志再落盘，清标志后、cfg.height 落盘前（本函数的窗口读取最长
/// HUD_WINDOW_OP_TIMEOUT）的竞态窗口内，poll_db 的下一 DB 拍（1-2 秒一拍，
/// 命中概率高）会读到旧配置（height 仍是 None / 旧值）→ 算自适应高度 →
/// should_sync_size（标志已 false）放行 → set_size 把窗口打回自适应值
///（松手后高度弹回 / 闪跳），本函数的落盘随后才写用户值，下一拍又
/// set_size 回来。先落盘后清标志则整个读取 + 落盘期间 poll_db 都被标志
/// 拦住；清标志时 cfg.height 已是用户值，后续 poll_db 的同值 set_size
/// 冗余无害。终值读取优先窗口实际尺寸（顺带覆盖 clamp / 取整差异：主线程
/// 窗口读取排在拖拽期间排队的 Resized 事件之后，不会读到滞后值），读不到
/// 时退回 Resized 挂点的内存槽；尺寸未变（点住热区没拖动）或低于最小尺寸
/// 不落盘（见纯函数 should_persist_resize_result）。落盘经
/// persist_hud_size 带拖前快照做高度轴条件写（纯拖宽不固化自适应高度）。
/// 落盘后还无条件用实际尺寸再 set_size 一次（WebView bounds 自愈，见
/// resize_end_persist）。收尾段递增尺寸纪元（HUD_SIZE_EPOCH）：feed 线程
/// 局部 last_size 记忆与窗口实际脱钩时，下一拍强制重同步一次。
/// async 命令 + run_on_main_thread + channel + 超时等待：同步命令占主线程，
/// 在其中调窗口 API 会自等待死锁（同 set_session_hud_config 的死锁防护
/// 说明）。
#[tauri::command]
pub async fn session_hud_resize_end(app: AppHandle) -> Result<(), String> {
    // 主体（读终值 → 判定 → 落盘 + WebView 自愈 set_size）在标志置位下
    // 执行；此处不提前清标志
    let outcome = resize_end_persist(&app);
    // 收尾统一清理（成败路径都到达，persist 失败也不能让标志永久留在
    // true——自适应高度会被永久禁用，见 HUD_USER_RESIZING）：先递增尺寸
    // 纪元（feed 线程下一拍作废局部 last_size 记忆、强制重同步一次
    // set_size，见 HUD_SIZE_EPOCH 的脱钩场景），再清标志，最后清拖前快照
    //（上方 persist 依赖快照做高度轴判定，必须在 persist 之后清）
    bump_size_epoch();
    set_user_resizing(false);
    clear_resize_snapshot();
    outcome
}

/// resize_end 的主体（读终值 → 判定 → 落盘 → WebView 自愈 set_size）：
/// 同步函数，由 async 命令包装后在非主线程上下文调用——内部 recv_timeout
/// 阻塞等待主线程，主线程同步命令中调用会自等待死锁。
fn resize_end_persist(app: &AppHandle) -> Result<(), String> {
    let actual = read_window_logical_size(app)?;
    let Some(size) = actual.or_else(|| hud_size_slot().lock().ok().and_then(|g| *g)) else {
        return Ok(());
    };
    if should_persist_resize_result(size, cached_size()) {
        // 立即落盘终值 + 记为程序侧目标（同尺寸回声不再重复落盘）。刻意不推进
        // HUD_SIZE_SAVED_AT 节流时钟：若本次读数恰好落后于仍在排队的最后一个
        // Resized 事件（事件队列先行、消息队列后行的窄竞态），该事件仍能立刻
        // 落盘真正的终值，不被节流吞掉；同尺寸回声由 LAST_SIZE 拦截，不会重复
        // 写盘
        persist_hud_size(size);
    }
    // WebView bounds 自愈（根治"HWND 600 / 视口 258"脱钩，成倍的 set_size
    // 才触发 WebView 重排、单靠 HWND 变化不够）：无条件用读到的实际尺寸再
    // set_size 一次——即使上方拒绝落盘（点住热区未拖动）也执行。同值
    // set_size 幂等无害，但会显式触发一次 WM_SIZE → wry 重设 WebView
    // bounds，把可能停在旧尺寸的 WebView 视口拉回与 HWND 一致。只用
    // actual（窗口真实读数）而非槽值兜底；actual 读取失败（None，事件
    // 循环异常）时跳过。仍在标志置位下执行，期间 poll_db 无干扰。
    if let Some(actual_size) = actual {
        force_webview_bounds_resync(app, actual_size);
    }
    Ok(())
}

/// 强制一次程序侧 set_size（WebView bounds 自愈专用，见 resize_end_persist）：
/// 经主线程事件循环执行并同步等待完成（run_on_main_thread + channel +
/// 超时兜底，与 read_window_logical_size 同款死锁防护口径——只能从非
/// 主线程上下文调用）。窗口不存在 / 投递失败静默跳过（自愈失败还有
/// HUD_SIZE_EPOCH 驱动的下一拍强制重同步兜底）。
fn force_webview_bounds_resync(app: &AppHandle, size: (f64, f64)) {
    let (tx, rx) = mpsc::channel::<()>();
    let app_main = app.clone();
    let posted = app.run_on_main_thread(move || {
        if let Some(win) = app_main.get_webview_window(SESSION_HUD_WINDOW_LABEL) {
            let _ = win.set_size(LogicalSize::new(size.0, size.1));
        }
        let _ = tx.send(());
    });
    if posted.is_ok() {
        // 超时仅事件循环异常时兜底：等待完成保证自愈 set_size 落在 resize_end
        // 清标志之前执行（期间 poll_db 无干扰）
        let _ = rx.recv_timeout(HUD_WINDOW_OP_TIMEOUT);
    }
}

/// 经主线程读取悬浮窗当前实际逻辑尺寸（宽, 高）：物理 outer_size ÷
/// scale_factor 换算逻辑值。窗口不存在 / scale 异常 / 超时（事件循环异常
/// 退出）返回 None，投递失败返回 Err。拖前快照（session_hud_resize_begin）
/// 与终值核校（resize_end_persist）共用同一读取口径。同步阻塞等待
///（HUD_WINDOW_OP_TIMEOUT 兜底），只能从非主线程上下文调用（async 命令
/// 体），主线程同步命令中调用会自等待死锁。
fn read_window_logical_size(app: &AppHandle) -> Result<Option<(f64, f64)>, String> {
    let (tx, rx) = mpsc::channel::<Option<(f64, f64)>>();
    let app_main = app.clone();
    app.run_on_main_thread(move || {
        let size = app_main
            .get_webview_window(SESSION_HUD_WINDOW_LABEL)
            .and_then(|win| {
                let scale = win.scale_factor().ok().filter(|s| *s > 0.0)?;
                let size = win.outer_size().ok()?;
                Some((size.width as f64 / scale, size.height as f64 / scale))
            });
        let _ = tx.send(size);
    })
    .map_err(|e| format!("投递会话悬浮窗尺寸读取失败: {e}"))?;
    // 超时（事件循环异常退出）：None，调用方自行退回内存槽等兜底
    Ok(rx.recv_timeout(HUD_WINDOW_OP_TIMEOUT).ok().flatten())
}

/// 保存并应用会话悬浮窗配置（改完即生效，无保存按钮）：
/// - 开关切换：enabled → 建窗 + 启轮询；!enabled → 停轮询 + 关窗；
/// - 其余字段变化：窗口存在时热推参数事件（页面即时应用透明度与
///   显示项；档位影响下一轮查询）。
///
/// 命令本体（set_session_hud_config）与悬浮窗内关闭按钮
/// （close_session_hud，只置 enabled = false 后走同一路径）共用本函数。
#[tauri::command]
pub async fn set_session_hud_config(
    config: SessionHudConfig,
    app: AppHandle,
) -> Result<SessionHudConfig, String> {
    apply_session_hud_config(app, config).await
}

/// apply 主体：见 set_session_hud_config 的 doc。
///
/// async + run_on_main_thread + channel + 超时等待：与 pet.rs 同款死锁
/// 防护（同步命令在 Windows 上占用主线程，建窗需要主线程事件循环处理
/// 消息，直接调窗口 API 会自等待死锁，pet.rs 有事故记载）。配置先落盘再
/// 动窗口：窗口操作失败配置不丢（下次启动 start_if_enabled 按 enabled
/// 恢复）；关闭分支先停轮询再落盘，Destroyed 复位路径
/// （handle_session_hud_window_destroyed）读到已关的开关不再改写，幂等。
async fn apply_session_hud_config(
    app: AppHandle,
    config: SessionHudConfig,
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

/// 程序侧最近一次设置的目标窗口逻辑尺寸（宽, 高）：轮询线程自适应
/// set_size / 建窗后记录，供窗口重建恢复与 Resized 挂点区分"程序自身
/// 的尺寸调整回声"（与目标一致 → 忽略不落盘）与"用户拖拽"（不一致 →
/// 持久化 width/height），避免自适应高度被误存成用户固定尺寸。
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

/// "用户正在拖拽热区调整尺寸"原子标志：悬浮窗自绘热区（前端 hud-resize.ts）
/// 的会话开始 / 结束经 session_hud_resize_begin / session_hud_resize_end 置
/// 位与清除。置位期间 poll_db 跳过程序侧 set_size——自适应高度模式每拍按
/// hud_height 公式 set_size，用户拖拽高度期间（pointerdown → up，秒级）会
/// 把高度拉回自适应值（闪跳 / 拖不动）。窗口销毁与重建路径兜底清除，防窗口
/// 重建后标志残留导致自适应高度永久失效。
static HUD_USER_RESIZING: AtomicBool = AtomicBool::new(false);

fn user_resizing() -> bool {
    HUD_USER_RESIZING.load(Ordering::Relaxed)
}

fn set_user_resizing(active: bool) {
    HUD_USER_RESIZING.store(active, Ordering::Relaxed);
}

/// 用户 Resized 活动宽限期（毫秒）：2.5 秒。构成 = 1 秒节流落盘间隔
///（HUD_SIZE_SAVE_THROTTLE_MS，拖拽中 persist 至少隔 1 秒才写盘）+
/// 松手后仍可能在事件队列里排队的最后一个 Resized 事件 + 原生
/// startResizeDragging 的 promise 在 Windows 上提前 settle 后用户仍在
/// 拖的整段（拖拽期间每帧 Resized 都会刷新锚点，宽限期从最后一帧起算，
/// 任意长的持续拖拽全程都被覆盖）。期间 poll_db 持续让路（见
/// user_size_active），既不与用户拖拽打架，也保证节流 persist 能把
/// 拖拽中的终值写盘。
const HUD_USER_RESIZE_GRACE_MS: u64 = 2_500;

/// 最后一次"用户 Resized 事件"到达时刻（毫秒，0 = 从未）：挂点
/// handle_session_hud_window_resized 对每个经 record_user_size 判定为
/// 用户尺寸的事件刷新（含拖拽进行中的每帧）。宽限期的锚点（见
/// HUD_USER_RESIZE_GRACE_MS）。
static HUD_LAST_USER_RESIZE_AT: AtomicU64 = AtomicU64::new(0);

fn mark_user_resized(now: u64) {
    HUD_LAST_USER_RESIZE_AT.store(now, Ordering::Relaxed);
}

fn last_user_resized_at() -> u64 {
    HUD_LAST_USER_RESIZE_AT.load(Ordering::Relaxed)
}

/// "用户尺寸活动期"判定（纯函数供单元测试）：HUD_USER_RESIZING 标志置位，
/// 或距最后用户 Resized 事件（HUD_LAST_USER_RESIZE_AT）不足宽限期。
/// poll_db 的 should_sync_size 与闭包双检 should_apply_size_now 都以本
/// 判定替代裸标志——根治"原生 promise 提前 settle → end 提前清标志 →
/// 用户继续拖期间 poll_db 抢先 set_size 打架 / 终值无人落盘"的竞态。
fn user_size_active(resizing: bool, last_resized_at: u64, now: u64) -> bool {
    resizing || now.saturating_sub(last_resized_at) < HUD_USER_RESIZE_GRACE_MS
}

/// 宽限期内出现过用户尺寸活动且尚未核校：Resized 挂点对每个经
/// record_user_size 判定为用户尺寸的事件置位（与 mark_user_resized 同处），
/// poll_db 在用户尺寸活动期结束后的下一拍消费（见 grace_check_due）——
/// 程序恢复尺寸干涉前的最后一次终值落盘机会，根治"原生 promise 提前
/// settle / 节流错过 / end 未再触发"一切终值丢失形态（松手后高度被旧
/// 配置拉回的根因兜底）。
static HUD_GRACE_PENDING: AtomicBool = AtomicBool::new(false);

fn grace_pending() -> bool {
    HUD_GRACE_PENDING.load(Ordering::Relaxed)
}

fn mark_grace_pending() {
    HUD_GRACE_PENDING.store(true, Ordering::Relaxed);
}

fn clear_grace_pending() {
    HUD_GRACE_PENDING.store(false, Ordering::Relaxed);
}

/// 宽限期到期核校触发判定（纯函数供单元测试）：用户尺寸活动期已结束
/// （!user_active——标志清除且距最后用户 Resized ≥ 宽限期）且宽限期内
/// 出现过用户尺寸活动且尚未核校（pending）→ 触发一次到期核校。核校本
/// 体（主线程读窗口实际尺寸 → 判定 → persist_hud_size 兜底落盘 → 清
/// pending → 重读配置覆盖局部 cfg）在 poll_db 内联执行，依赖 AppHandle
/// 与主线程事件循环不可单元测试，触发时序拆由本函数承担。
fn grace_check_due(user_active: bool, pending: bool) -> bool {
    !user_active && pending
}

/// 拖前尺寸快照（宽, 高，逻辑值）：session_hud_resize_begin 置位标志时
/// 读取当前窗口实际尺寸存入，session_hud_resize_end / 窗口销毁 / 建窗
/// 路径清除。用途（高度轴方向感知）：用户纯拖宽度（West 向，高度不动）
/// 时 Resized 事件的高度 = 拖前自适应值，persist_hud_size 若无条件写
/// cfg.height 会把自适应高度固化成用户固定高度（会话再多高度也不涨，
/// 用户感知"高度有 bug"的一种形态）；落盘时对照快照——高度与快照一致
///（容差内，用户没动高度轴）则 cfg.height 保持原样（None 仍自适应 /
/// Some 仍原用户值），见 merge_persist_axes。begin 的快照读取是异步的，
/// 用户飞快拖拽时可能晚于首个 Resized 落盘——该窗口内 persist 退化为
/// 现状行为（把当时高度写死），属可接受窄边缘。
static HUD_RESIZE_SNAPSHOT: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();

fn resize_snapshot_slot() -> &'static Mutex<Option<(f64, f64)>> {
    HUD_RESIZE_SNAPSHOT.get_or_init(|| Mutex::new(None))
}

/// 读拖前快照；None = 会话外 / begin 读取失败（放弃方向感知）
fn resize_snapshot() -> Option<(f64, f64)> {
    resize_snapshot_slot().lock().ok().and_then(|g| *g)
}

fn store_resize_snapshot(size: (f64, f64)) {
    if let Ok(mut guard) = resize_snapshot_slot().lock() {
        *guard = Some(size);
    }
}

fn clear_resize_snapshot() {
    if let Ok(mut guard) = resize_snapshot_slot().lock() {
        *guard = None;
    }
}

/// 尺寸差容差（逻辑 px）：系统窗口读数与程序侧目标值之间存在取整 / 缩放
/// 换算残差（逻辑值 → 物理像素四舍五入 → 再除以 scale_factor；系统缩放
/// 非 100% 时回除未必复原），容差内一律视为"与程序侧目标一致的尺寸"。
/// 两处判定共用：热区会话终值是否落盘（点住热区未拖动）、Resized 挂点
/// 区分自身 set_size 回声（见 size_matches_target）
const HUD_SIZE_EPSILON: f64 = 1.0;

/// 程序侧尺寸同步判定（纯函数供单元测试）：与上次目标尺寸不同才需要
/// set_size；用户尺寸活动期（user_resizing 入参为"标志置位 或 宽限期内"
/// 的合成判定，见 user_size_active）一律跳过
fn should_sync_size(
    user_resizing: bool,
    last: Option<(f64, f64)>,
    size: (f64, f64),
) -> bool {
    if user_resizing {
        return false;
    }
    last.map(|s| s != size).unwrap_or(true)
}

/// poll_db 投递闭包内的双检判定（纯函数供单元测试）：闭包从投递到在
/// 主线程实际执行存在时间窗，期间用户可能按下热区（session_hud_resize_
/// begin 的标志置位晚于上方 should_sync_size 检查到达）。执行时复查一次
/// 用户活动判定（user_resizing 入参为 user_size_active 的合成结果，闭包
/// 执行时重新取时刻），活动期则放弃本次 set_size——消除"排队期间用户
/// 开始拖拽 / 宽限期内用户仍在拖"的竞态窗口（set_size 仍会把高度拉回
/// 自适应公式值，表现为拖拽起始阶段高度回弹 / 首拍抖动）。闭包里拿不到
/// last_size 快照语义，只做活动判定复查，尺寸比对仍由调用侧的
/// should_sync_size 承担
fn should_apply_size_now(user_resizing: bool) -> bool {
    !user_resizing
}

/// 尺寸是否与程序侧目标一致（容差内，纯函数供单元测试复用）：Resized
/// 挂点判"自身 set_size 回声"与热区终值落盘判定共用同一口径。逻辑值到
/// 物理像素的取整 + 除以 scale_factor 会让回声与目标差出亚像素残差
///（系统缩放 125% / 150% 时 1.25L / 1.5L 取整后回除不复原原值），精确
/// 相等判定会漏判 → 回声被当成用户尺寸落盘 → 自适应高度模式静默冻结
fn size_matches_target(size: (f64, f64), target: Option<(f64, f64)>) -> bool {
    target.is_some_and(|(w, h)| {
        (w - size.0).abs() <= HUD_SIZE_EPSILON && (h - size.1).abs() <= HUD_SIZE_EPSILON
    })
}

/// 热区会话终值是否落盘（纯函数供单元测试）：低于最小尺寸（最小化 /
/// 系统抖动）不落；与程序侧目标尺寸一致（点住热区未拖动）不落——否则
/// 一次点击就把自适应高度模式误冻结成用户固定尺寸
fn should_persist_resize_result(size: (f64, f64), last_target: Option<(f64, f64)>) -> bool {
    if size.0 < HUD_MIN_WIDTH || size.1 < HUD_MIN_HEIGHT {
        return false;
    }
    !size_matches_target(size, last_target)
}

/// 拖拽尺寸落盘的轴向合并（纯函数供单元测试）：宽度轴无条件写（宽度本就
/// 是纯用户语义，无自适应）；高度轴对照拖前快照（HUD_RESIZE_SNAPSHOT）
/// 条件写——仅当高度相对拖前快照变化超过容差（用户确实动了高度轴）才写
/// 死用户值，否则保持 cfg.height 原样（None 仍自适应 / Some 仍原用户值，
/// 纯拖宽度不固化自适应高度）。快照缺失（begin 读取失败 / 快照晚于首个
/// Resized 的窄竞态）时退化为现状行为（两轴都写）。返回 (width, height)
/// 的落盘目标值。
fn merge_persist_axes(
    cfg_height: Option<f64>,
    size: (f64, f64),
    snapshot: Option<(f64, f64)>,
) -> (f64, Option<f64>) {
    let height = match snapshot {
        // 高度未动（容差内，含取整 / 缩放换算残差）：不覆写 cfg.height
        Some(snap) if (size.1 - snap.1).abs() <= HUD_SIZE_EPSILON => cfg_height,
        // 高度动了 / 快照缺失：写死用户值（现状行为）
        _ => Some(size.1),
    };
    (size.0, height)
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

/// 尺寸同步纪元计数器：session_hud_resize_end 收尾段与建窗路径
/// （ensure_session_hud_window）递增。动机：poll_db 的尺寸同步基准是
/// feed 线程**局部的 last_size（记忆值）而非窗口实际值**——用户拖到
/// 600 而 last_size 仍是 (w,258) 时，下一拍 should_sync_size 比较
/// "记忆 == 目标"直接跳过，HWND 实际尺寸与程序认知脱钩（窗口停在
/// 600 空壳、WebView 视口不跟随、空态触发一次 set_size 才瞬间缩回的
/// 根源）。纪元变化令 feed 线程把局部 last_size 置 None，下一拍
/// should_sync_size 必然放行一次 set_size：cfg.height 已落盘时目标 =
/// 用户值（HWND 与配置强制一致），未落盘时目标 = 自适应值（HWND 回
/// 内容高度）——两种都消除脱钩态。
static HUD_SIZE_EPOCH: AtomicU64 = AtomicU64::new(0);

fn bump_size_epoch() {
    HUD_SIZE_EPOCH.fetch_add(1, Ordering::Relaxed);
}

fn size_epoch() -> u64 {
    HUD_SIZE_EPOCH.load(Ordering::Relaxed)
}

/// 窗口高度（逻辑 px，纯函数供单元测试复用）：头部 + 会话行 × 条数
/// + 列表区边框 + 模型速度区行数与边框 + 今日合计行 + 折叠提示行 + 底
/// 部留白，出口整体乘 font_scale（页面全部字号/行高/行高栅格随
/// --hud-scale calc 缩放，公式同步缩放保持内容恰好铺满、不被裁）；脏
/// font_scale 按默认 1.0 处理。空列表回空态最小高（显示"暂无活跃会话"，
/// 不为 0 高；空态不显示模型速度区与今日行——两者都跟随列表存在）。
/// 折叠提示行只在列表确实达到上限条数时出现、今日合计行仅在 today 为
/// 真（开启数据行显示、今日有请求且有可见会话，见 today_visible）、模
/// 型速度区仅在 model_rows > 0（调用方传 model_speed_visible 判定后的
/// 行数，开启数据行 + 有样本 + 有可见会话）时计入。列表区与模型速度区
/// 各有 1px 容器 border-bottom（分隔线/收边，CSS 无 box-sizing 包含，
/// 见 HUD_LIST_BORDER_H / HUD_MODELS_BORDER_H 注释——V3 修复窗口比内
/// 容矮 1~3px 导致会话行被裁的高度 Bug）。与 session-hud.html 的 CSS
/// 尺寸常量一一对应。
fn hud_height(visible_rows: usize, folded: bool, today: bool, model_rows: usize, font_scale: f64) -> f64 {
    let scale = if font_scale.is_finite()
        && (HUD_FONT_SCALE_RANGE.0..=HUD_FONT_SCALE_RANGE.1).contains(&font_scale)
    {
        font_scale
    } else {
        HUD_FONT_SCALE_DEFAULT
    };
    let base = if visible_rows == 0 {
        HUD_EMPTY_HEIGHT
    } else {
        let rows = visible_rows.min(MAX_VISIBLE_SESSIONS) as f64;
        let more = folded && visible_rows >= MAX_VISIBLE_SESSIONS;
        HUD_HEADER_H
            + rows * HUD_ROW_H
            + HUD_LIST_BORDER_H
            + if more { HUD_MORE_H } else { 0.0 }
            + if today { HUD_TODAY_H } else { 0.0 }
            + if model_rows > 0 {
                model_rows.min(HUD_MAX_MODEL_ROWS) as f64 * HUD_MODEL_ROW_H + HUD_MODELS_BORDER_H
            } else {
                0.0
            }
            + HUD_BOTTOM_PAD
    };
    base * scale
}

/// 程序侧目标窗口尺寸（逻辑 px，纯函数供单元测试复用）：建窗 / 每拍
/// set_size 的入参与 LAST_SIZE、HUD_SIZE 回声比对的统一基准，按窗口侧
/// 实际生效口径收口两件事——取整 + 夹到合法域：
/// - 取整：LogicalSize 的逻辑值到物理像素按 scale_factor 乘后四舍五入，
///   font_scale（0.8~1.4 步进 0.05）让 hud_height 出现 .25 / .75 等小数
///   （如 95 × 1.05 = 99.75），回声取整后与小数目标精确相等判定不成立
///   → 非默认字号下自适应高度被 Resized 挂点误判成用户尺寸落盘，静默变
///   固定尺寸；
/// - 最小高：建窗 min_inner_size（HUD_MIN_HEIGHT = 160）会钳住低于它的
///   set_size（空态 64、单行 95 都在其下），不夹取则程序侧目标永远对不
///   上回声（同上误判）。
fn hud_target_size(width: f64, height: f64) -> (f64, f64) {
    (
        width.round().clamp(HUD_MIN_WIDTH, HUD_MAX_WIDTH),
        height.round().clamp(HUD_MIN_HEIGHT, HUD_MAX_HEIGHT),
    )
}

/// 今日合计行显隐判定（纯函数供单元测试复用，轮询线程与前端
/// session-hud-main.ts renderShell 的 showToday 判定同条件）：开启
/// 数据行显示（show_tokens）+ 今日有请求 + 有可见会话行（空态不
/// 显示——今日行跟随列表存在）。show_tokens 的语义是"不想看数字"，
/// 今日行整行数字同理一并隐藏，窗口高度随之增减。
fn today_visible(show_tokens: bool, today_total: i64, visible_sessions: usize) -> bool {
    show_tokens && today_total > 0 && visible_sessions > 0
}

/// 模型速度区显隐判定（纯函数供单元测试复用，轮询线程与前端
/// renderShell 的 showModels 判定逐字同条件）：开启数据行显示
///（show_tokens）+ 窗口内有可信速度样本（model_speeds 非空）+ 有可见
/// 会话行（空态不显示——模型速度区与今日行一样跟随列表存在）。
/// 速度属数字信息，随 show_tokens 一并隐藏，窗口高度随之增减。
fn model_speed_visible(show_tokens: bool, model_count: usize, visible_sessions: usize) -> bool {
    show_tokens && model_count > 0 && visible_sessions > 0
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

/// 确保 "session-hud" 悬浮窗存在并应用配置：已存在时仅热推参数（防重
/// 复创建同名窗口；窗口尺寸由用户拖拽 Resized 挂点与轮询线程自适应
/// 维护，此处不再 set_size——设置卡的宽度滑块已移除，配置宽度只随拖拽
/// 变化，重复同步反而可能与拖拽竞态）；不存在时创建（透明、无边框、
/// 置顶、不抢焦点、skipTaskbar、shadow 关闭、**可拖拽调整大小**，位置
/// 取持久化坐标或默认主显示器右下角）。必须在主线程的事件循环上下文
/// 调用（WebviewWindowBuilder 的要求）：合法调用点是 setup 阶段
///（start_if_enabled）与 run_on_main_thread 投递的闭包
///（set_session_hud_config）。不能在同步命令主体里直接调——同步命令
/// 占用主线程，建窗等不到事件循环处理消息，自等待死锁（见 pet.rs 同名
/// 注释的事故记载）。
fn ensure_session_hud_window(app: &AppHandle, cfg: &SessionHudConfig) -> Result<(), String> {
    if app.get_webview_window(SESSION_HUD_WINDOW_LABEL).is_some() {
        // 已存在：仅热推参数（尺寸维护见函数 doc）
        push_session_hud_params(app, cfg);
        return Ok(());
    }

    // 建窗初始尺寸：用户拖拽过（height 已持久化）→ 按用户尺寸恢复；
    // 自适应模式 → 高度沿用最后已知高度（关后再开恢复原列表高度），
    // 无记录时空态最小高（首帧即"暂无活跃会话"，2 秒内数据到达后由
    // 轮询线程自适应）。经 hud_target_size 取整 + 夹取：建窗尺寸即
    // LAST_SIZE 基准，回声比对不受小数与 min_inner_size 钳制影响
    let (w, h) = hud_target_size(
        cfg.width,
        cfg.height
            .or_else(|| cached_size().map(|(_, h)| h))
            .unwrap_or_else(|| hud_height(0, false, false, 0, cfg.font_scale)),
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
    // 可自由拖拽调整大小（V3 起）：用户拖出尺寸经 Resized 挂点节流
    // 持久化到 session-hud.json 的 width/height，内容超高时会话列表
    // 纵向滚动（见 session-hud.html #hud-list）
    .resizable(true)
    // Windows 钳宽修复（照搬 pet.rs）：tao 给所有窗口无条件带
    // WS_CAPTION，DefWindowProc 的 WM_GETMINMAXINFO 默认最小跟踪宽度
    // 会把小窗口出生即钳宽，min_inner_size 在子类化后覆盖该值；同时
    // 承担自由拖拽的最小尺寸约束（260×160，与 HUD_MIN_WIDTH/
    // HUD_MIN_HEIGHT、Resized 落盘下限一致）
    .min_inner_size(HUD_MIN_WIDTH, HUD_MIN_HEIGHT)
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
    // 同步递增尺寸纪元：feed 线程（含被复用的旧线程 / 重启后的新线程）的
    // 局部 last_size 是旧窗口的记忆，与建窗尺寸恰好相等时会让
    // should_sync_size 误判"已同步"跳过 set_size；作废记忆保证首拍必与
    // 配置强制对齐一次（见 HUD_SIZE_EPOCH）
    bump_size_epoch();
    // 清"用户调整中"标志与拖前快照兜底：拖拽会话中窗口被销毁 / 重建时
    // 前端来不及调 session_hud_resize_end，标志残留会让自适应高度永久失效
    //（见 HUD_USER_RESIZING）；旧会话快照残留会污染新窗口首次拖拽的高度
    // 轴判定（新窗口尺寸与旧快照无关联）
    set_user_resizing(false);
    clear_resize_snapshot();

    // 首帧参数：页面加载后也会主动 get_session_hud_config，这里推送
    // 保证先到（双通道幂等）
    push_session_hud_params(app, cfg);
    Ok(())
}

/// 向悬浮窗推送当前显示参数（透明度 + 显示项 + 活跃档位 + 字体缩放）。
/// 透明度由页面经 CSS opacity 应用（窗口级 set_opacity 平台差异大，CSS
/// 路径跨平台一致且作用于内容层）；字体缩放经 CSS 变量 --hud-scale 应
/// 用并同步设置面板字体滑块刻度与透明度滑块读数。
fn push_session_hud_params(app: &AppHandle, cfg: &SessionHudConfig) {
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct HudParams {
        opacity: f64,
        window_minutes: u32,
        show_tokens: bool,
        show_model: bool,
        font_scale: f64,
    }
    let _ = app.emit_to(
        SESSION_HUD_WINDOW_LABEL,
        SESSION_HUD_PARAMS_EVENT,
        HudParams {
            opacity: cfg.opacity,
            window_minutes: cfg.window_minutes,
            show_tokens: cfg.show_tokens,
            show_model: cfg.show_model,
            font_scale: cfg.font_scale,
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

/// 悬浮窗 Destroyed 事件挂点：冲刷最终位置与尺寸（无节流）+ 停轮询 +
/// 开关复位（窗口没了 = 功能关闭，防面板显示与实况脱节；用户 alt+F4 等
/// 旁路关闭后开关能如实回读为关）。
pub fn handle_session_hud_window_destroyed(_app: &AppHandle) {
    stop_feed();
    // 递增窗口代数：销毁后快速重开时旧 feed 线程可能被复用（sleep 窗口
    // 内未及感知 stop），代数变化令其清空变化检测缓存重发一帧，新页面
    // 不停留在假空态（见 WINDOW_EPOCH 注释）
    bump_window_epoch();
    // 清"用户调整中"标志兜底：拖拽会话中窗口被销毁（前端不再有机会调
    // session_hud_resize_end），标志残留会让下次建窗后的自适应高度失效
    set_user_resizing(false);
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    // 冲刷最终位置（无节流）
    let pos = hud_pos_slot().lock().ok().and_then(|guard| *guard);
    if let Some(p) = pos {
        HUD_POS_SAVED_AT.store(now, Ordering::Relaxed);
        persist_hud_pos(p);
    }
    // 冲刷最终尺寸（用户拖拽的自由宽高，无节流；高度轴对照拖前快照条件
    // 写——拖拽中销毁时槽里是拖后值、快照是拖前值，方向判定仍成立）
    let size = hud_size_slot().lock().ok().and_then(|guard| *guard);
    if let Some(sz) = size {
        HUD_SIZE_SAVED_AT.store(now, Ordering::Relaxed);
        persist_hud_size(sz);
    }
    // 拖前快照随窗口销毁清理：须在上方冲刷落盘之后（persist 依赖它做
    // 高度轴判定），残留快照会污染下次建窗后首次拖拽的判定
    clear_resize_snapshot();
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
// 窗口尺寸持久化（Resized 事件节流落盘，自由拖拽宽高，V3 起）
// ============================================================

/// 最近一次收到的窗口逻辑尺寸（逻辑坐标）：Resized 事件高频触发（拖拽
/// 边缘时连续），先写内存，按节流间隔落盘
static HUD_SIZE: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
/// 上次尺寸落盘时刻（毫秒），0 = 从未落盘
static HUD_SIZE_SAVED_AT: AtomicU64 = AtomicU64::new(0);
/// 尺寸落盘节流间隔（毫秒）：拖拽结束后最迟 1 秒内持久化（与 Moved
/// 同口径）
const HUD_SIZE_SAVE_THROTTLE_MS: u64 = 1000;

fn hud_size_slot() -> &'static Mutex<Option<(f64, f64)>> {
    HUD_SIZE.get_or_init(|| Mutex::new(None))
}

/// 把 Resized 事件尺寸记入内存槽（判定 + 写槽集中处，纯判定复用
/// should_persist_resize_result，供单元测试直接驱动）：槽只承载"用户产生
/// 的尺寸"——低于最小尺寸（最小化 / 系统抖动）与程序侧 set_size 回声
///（与目标尺寸容差内一致，见 size_matches_target）都不进槽，也不覆盖槽里
/// 已有的用户尺寸。返回是否视为用户尺寸（调用方据此决定是否节流落盘）。
///
/// 回声过滤是关窗冲刷安全的前提：Destroyed 挂点
///（handle_session_hud_window_destroyed）无节流冲刷落盘时直接读槽，回声
/// 一旦进槽，关窗（关开关 / 关窗口）就会把程序侧尺寸当成"用户拖拽出的
/// 尺寸"写进 session-hud.json 的 width/height，自适应高度模式被永久冻结；
/// 过滤后冲刷落盘的不是用户尺寸就是槽为空不写。
fn record_user_size(size: (f64, f64), target: Option<(f64, f64)>) -> bool {
    if !should_persist_resize_result(size, target) {
        return false;
    }
    if let Ok(mut guard) = hud_size_slot().lock() {
        *guard = Some(size);
    }
    true
}

/// 悬浮窗 Resized 事件挂点（lib.rs 的 on_window_event 转发）：把用户
/// 自由拖拽出的窗口尺寸（物理坐标转逻辑）节流持久化到 session-hud.json
/// 的 width/height。三类事件必须忽略：
/// 1. 与程序侧目标尺寸（LAST_SIZE）一致的回声——轮询线程自适应高度 /
///    建窗 set_size 自身触发（含取整 / 缩放换算的亚像素残差，见
///    size_matches_target），绝不能存成用户尺寸（否则自适应模式被
///    一次数据变化永久打断）；
/// 2. 低于最小尺寸（Windows 最小化等系统抖动）——不可用尺寸不落盘；
/// 3. 与上次已落盘值相同（重复事件）。
/// 前两类同时由 record_user_size 拦在内存槽之外（写内存都不写，防止
/// Destroyed 前的冲刷把非用户尺寸带走）。落盘后同步 LAST_SIZE（该尺寸
/// 即新的"程序侧目标"），后续同尺寸回声不再重复落盘。
pub fn handle_session_hud_window_resized(win: &tauri::Window, size: tauri::PhysicalSize<u32>) {
    let scale = win.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return;
    }
    let logical = (size.width as f64 / scale, size.height as f64 / scale);
    // 只有用户动手产生的尺寸才进槽并进入落盘流程（最小尺寸下限与回声
    // 过滤见 record_user_size）
    if !record_user_size(logical, cached_size()) {
        return;
    }
    // 节流落盘：拖拽期间每秒最多一次写盘
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    // 刷新"最后用户 Resized 时刻"（含拖拽进行中的每帧）：宽限期锚点，
    // 见 HUD_USER_RESIZE_GRACE_MS / user_size_active——原生 promise 提前
    // settle 后用户继续拖的整段，靠每帧刷新的锚点让 poll_db 持续让路
    mark_user_resized(now);
    // 置位"宽限期待核校"：本帧已判定为用户尺寸（record_user_size 通过），
    // 宽限期一过 poll_db 需要一次到期核校兜底落盘终值（见 HUD_GRACE_
    // PENDING / grace_check_due——节流 persist 与 resize_end 都可能错过
    // 用户拖的最后一段终值，此处保证宽限期内只要有过用户尺寸活动就必有
    // 一次核校；置位幂等，拖拽每帧重复置无害）
    mark_grace_pending();
    let last = HUD_SIZE_SAVED_AT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < HUD_SIZE_SAVE_THROTTLE_MS {
        return;
    }
    HUD_SIZE_SAVED_AT.store(now, Ordering::Relaxed);
    persist_hud_size(logical);
}

/// 把用户拖拽尺寸合并进 session-hud.json（保留其它字段）。宽度轴无条件
/// 写；高度轴经 merge_persist_axes 对照拖前快照条件写——纯拖宽度（高度
/// 与拖前一致）不把当时的自适应高度固化成用户固定高度（见该函数 doc）。
/// 与已持久化值相同（宽度与高度都一致）则只更新内存目标不写盘。落盘失败
/// 静默（下次 Resized 再试）；成功后把当前实际尺寸记为程序侧目标
///（LAST_SIZE），避免同尺寸回声反复落盘。
fn persist_hud_size(size: (f64, f64)) {
    let mut cfg = load_session_hud_config();
    let (width, height) = merge_persist_axes(cfg.height, size, resize_snapshot());
    if cfg.width == width && cfg.height == height {
        remember_size(size);
        return;
    }
    cfg.width = width;
    cfg.height = height;
    if save_session_hud_config(&cfg).is_ok() {
        remember_size(size);
    }
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
    // 尺寸同步纪元基线：resize_end 收尾 / 建窗路径递增后，局部 last_size
    // 记忆与窗口实际可能脱钩（见 HUD_SIZE_EPOCH），作废记忆保证下一拍
    // poll_db 必然重同步一次 set_size
    let mut size_epoch_base = size_epoch();
    // rollout 旁路速度监控器与最后快照（速度拍在其上原地更新 speed）
    let mut monitor = RolloutMonitor::default();
    let mut last_snapshot: Option<HudSnapshot> = None;
    let mut last_fallback: BTreeMap<String, Option<SpeedSnapshot>> = BTreeMap::new();
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
        let now_size_epoch = size_epoch();
        if now_size_epoch != size_epoch_base {
            size_epoch_base = now_size_epoch;
            // 记忆与窗口实际脱钩（拖拽终值 / 新建窗尺寸），置 None 令下一拍
            // should_sync_size 必然放行一次 set_size（强制 HWND/WebView 与
            // 配置对齐，见 HUD_SIZE_EPOCH）
            last_size = None;
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
    last_fallback: &mut BTreeMap<String, Option<SpeedSnapshot>>,
) {
    let result = (|| -> Result<(), String> {
        let mut cfg = load_session_hud_config().clamped();
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let (mut snapshot, trees) =
            collect_session_snapshot(&conn, cfg.window_minutes, now_ms)?;

        // rollout 旁路速度：先同步树并观察当前轮身份，再增量读新完成
        // 请求，最后做展示决策。必须先 observe_round：快速连续发问时，
        // 新轮 rollout 可能已在本次 DB 拍前追加；若先 ingest，随后清理
        // 旧轮会把这条新轮参考值一并丢掉。
        // 仅覆盖速度字段；累计字段与 rollout 无关（防双计）。DB 速度在
        // 无可靠一对一请求 ID 时按完成时刻择优，rollout 只在 DB 尚未出现
        // 或明确更晚时作参考。
        monitor.sync_trees(&trees, rollout_dir().as_deref());
        last_fallback.clear();
        for s in &mut snapshot.sessions {
            if let Some(tree) = monitor.tree_mut(&s.session_id) {
                tree.observe_round(
                    s.round_key.as_deref(),
                    s.round_started_at,
                    s.generating,
                );
            }
        }
        monitor.ingest(now_ms);
        for s in &mut snapshot.sessions {
            let fallback = brief_speed_snapshot(s);
            last_fallback.insert(s.session_id.clone(), fallback.clone());
            if let Some(tree) = monitor.tree_mut(&s.session_id) {
                let chosen = tree.display_speed(s.generating, fallback, now_ms);
                apply_speed_snapshot(s, chosen);
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

        // 高度自适应（仅"从未拖拽"模式）：会话条数（含折叠态）变化 →
        // 窗口高度随之调整（乘 font_scale 与页面缩放栅格一致）。用户
        // 拖拽过（height 已持久化）→ 窗口尺寸归用户，此处只读不动；
        // 内容超高时会话列表区纵向滚动（页面 CSS flex 布局自动处理，
        // 模型速度区/今日行/头部固定不滚）。必须在主线程操作窗口（轮
        // 询线程非主线程，直接调窗口 API 会 panic）；投递失败静默（下
        // 轮条数变化时再试）。
        let visible = last_snapshot
            .as_ref()
            .map(|s| s.sessions.len())
            .unwrap_or(0)
            .min(MAX_VISIBLE_SESSIONS);
        let total_active = last_snapshot
            .as_ref()
            .map(|s| s.total_active)
            .unwrap_or(0);
        // 今日合计行显隐：开启数据行显示（show_tokens）+ 今日有请求 +
        // 有可见会话行（空态不显示，今日行跟随列表存在；判定与前端
        // renderShell 的 showToday 一致，见 today_visible）
        let today = last_snapshot.as_ref().is_some_and(|s| {
            today_visible(cfg.show_tokens, s.today_total.total, s.sessions.len())
        });
        // 模型速度区行数：显隐判定与前端 renderShell 的 showModels 逐字
        // 同条件（见 model_speed_visible），不显示按 0 行计（窗口高度不
        // 含该区）
        let model_rows = last_snapshot
            .as_ref()
            .map(|s| {
                if model_speed_visible(cfg.show_tokens, s.model_speeds.len(), s.sessions.len()) {
                    s.model_speeds.len()
                } else {
                    0
                }
            })
            .unwrap_or(0);
        // 用户尺寸活动期（标志置位 或 距最后用户 Resized < 宽限期，见
        // user_size_active）时跳过：自适应高度模式的每拍 set_size 会把用户
        // 拖出的高度拉回公式值（闪跳 / 拖不动）。宽限期兜住原生
        // startResizeDragging 的 promise 提前 settle 竞态——end 被提前触发
        // 清了标志，但用户仍在拖（每帧 Resized 刷新锚点），期间 poll_db
        // 必须持续让路，节流 persist 才能把拖拽中的终值写盘（否则终值
        // 永远丢失）。拖拽结束由 session_hud_resize_end 清标志并即时落盘
        // 核校
        let user_active = user_size_active(
            user_resizing(),
            last_user_resized_at(),
            now_ms.max(0) as u64,
        );
        // 宽限期到期核校（终值兜底落盘，触发判定见 grace_check_due /
        // HUD_GRACE_PENDING）：宽限期内出现过用户尺寸活动且活动期已结束
        // → 恢复程序侧尺寸干涉（下方 set_size）之前先核校一次终值。兜底
        // 动机：N 向 startResizeDragging 的 promise 在 Windows 上可能提前
        // settle → resize_end 提前执行（提前 persist 当时高度、bump 纪元、
        // 清标志/快照）→ 用户继续拖的最后一段（距上次节流 persist < 1 秒、
        // 松手后没有第二次 end）终值无人落盘 → 宽限期一过，下方按旧
        // cfg.height 算出的 set_size 会把窗口高度拉回（有数据时松手弹回
        // 的根因）。核校动作：经主线程读窗口实际尺寸（与
        // resize_end_persist 同一读取口径），确为用户拖出的值（不低于
        // 最小尺寸且与程序侧目标 LAST_SIZE 差超容差，即非程序回声）才
        // persist_hud_size 落盘终值；无论是否落盘，核校完成即消费
        // pending。落盘改了配置，本拍后续的尺寸计算必须用新值——重读
        // 配置覆盖局部 cfg（此后目标 == 实际，下方 set_size 即便放行也是
        // 同值幂等，窗口保持用户高度；"核校后重读 cfg"依赖主线程窗口
        // 读取，不可单元测试，由 grace_check_due 锁定时序）。读失败
        // （事件循环异常超时）同样清 pending 后走本拍正常逻辑（下次
        // Resized 会重新置位，重试机会天然存在）。与 resize_end 的正常
        // 核校路径幂等共存：persist 同值时相同值比较直接跳过写盘
        if grace_check_due(user_active, grace_pending()) {
            let actual = read_window_logical_size(app).ok().flatten();
            if let Some(size) = actual
                .filter(|&s| should_persist_resize_result(s, cached_size()))
            {
                persist_hud_size(size);
            }
            clear_grace_pending();
            cfg = load_session_hud_config().clamped();
        }
        let height = cfg.height.unwrap_or_else(|| {
            hud_height(
                visible,
                total_active > MAX_VISIBLE_SESSIONS,
                today,
                model_rows,
                cfg.font_scale,
            )
        });
        // 目标尺寸经 hud_target_size 取整 + 夹取后再 set_size：LogicalSize
        // 小数（font_scale 让公式出 .25 / .75）与 min_inner_size 钳制都会
        // 让回声与目标不再精确相等，被 Resized 挂点误判成"用户尺寸"落盘 →
        // 自适应高度模式静默变固定尺寸
        let size = hud_target_size(cfg.width, height);
        if should_sync_size(user_active, *last_size, size) {
            *last_size = Some(size);
            remember_size(size);
            // 两个句柄：一个供方法调用（借用于调用期间），一个供闭包捕获
            let app_task = app.clone();
            let app_call = app.clone();
            let _ = app_call.run_on_main_thread(move || {
                // 双检用户活动判定（V2 竞态消除，判定见 should_apply_
                // size_now）：闭包从投递到主线程实际执行存在时间窗，期间
                // 用户按下热区 / 仍处宽限期的话此处以执行时刻重新合成判定，
                // 活动期则放弃本次 set_size（防排队闭包把拖拽起始阶段的
                // 高度拉回公式值 / promise 提前 settle 后与继续拖的用户打架）
                if should_apply_size_now(user_size_active(
                    user_resizing(),
                    last_user_resized_at(),
                    chrono::Utc::now().timestamp_millis().max(0) as u64,
                )) {
                    if let Some(win) = app_task.get_webview_window(SESSION_HUD_WINDOW_LABEL) {
                        let _ = win.set_size(LogicalSize::new(size.0, size.1));
                    }
                }
            });
        }
        Ok(())
    })();
    let _ = result; // 静默跳过本轮（库被锁超时/文件缺失等瞬态），下个周期重试
}

fn brief_speed_snapshot(brief: &HudSessionBrief) -> Option<SpeedSnapshot> {
    Some(SpeedSnapshot {
        value: brief.speed?,
        quality: brief.speed_quality?,
        completed_at: brief.speed_completed_at?,
        request_id: None,
    })
}

fn apply_speed_snapshot(brief: &mut HudSessionBrief, snapshot: Option<SpeedSnapshot>) {
    brief.speed = snapshot.as_ref().map(|speed| speed.value);
    brief.speed_quality = snapshot.as_ref().map(|speed| speed.quality);
    brief.speed_completed_at = snapshot.as_ref().map(|speed| speed.completed_at);
    brief.speed_state = if brief.generating {
        if snapshot.is_some() {
            SpeedState::Recent
        } else {
            SpeedState::Measuring
        }
    } else if snapshot.is_some() {
        SpeedState::Recent
    } else {
        SpeedState::Unavailable
    };
}

/// 速度拍（与 DB 拍交替，1 秒一次）：仅增量发现 rollout 中的新完成请求，
/// 不查库、不重算或衰减已有速度。速度变化并入快照变化检测；其余字段仍
/// 随 DB 拍 2 秒更新，内容不变不 emit。
fn poll_speed(
    app: &AppHandle,
    cache: &mut Option<String>,
    monitor: &mut RolloutMonitor,
    last_snapshot: &mut Option<HudSnapshot>,
    last_fallback: &BTreeMap<String, Option<SpeedSnapshot>>,
) {
    let Some(snapshot) = last_snapshot.as_mut() else {
        return; // 首个 DB 拍尚未成功，无内容可更新
    };
    let now_ms = chrono::Utc::now().timestamp_millis();
    monitor.ingest(now_ms);
    for s in &mut snapshot.sessions {
        if let Some(tree) = monitor.tree_mut(&s.session_id) {
            let fallback = last_fallback.get(&s.session_id).cloned().flatten();
            let chosen = tree.display_speed(s.generating, fallback, now_ms);
            apply_speed_snapshot(s, chosen);
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
    /// 最新模型请求速度（t/s）。generation 是首字到完成的可信速度，
    /// request_average 是只有请求总耗时的近似值，前端显示 `≈`；无效
    /// 或尚未完成请求时为 None。
    pub speed: Option<f64>,
    /// 速度质量，与 speed 一一对应。
    pub speed_quality: Option<SpeedQuality>,
    /// 速度所属请求的有效完成时刻，用于 rollout 先到、DB 后到时择优。
    pub speed_completed_at: Option<i64>,
    /// 速度状态：生成中且尚无本轮完成请求时为 measuring。
    pub speed_state: SpeedState,
    /// 当前轮可靠身份：优先 user message id，缺失时回退最新 turn id。
    /// 仅供悬浮版速度状态机使用，不进入 IPC payload。
    #[serde(skip)]
    pub round_key: Option<String>,
    /// 当前轮起始参考时刻（user message/turn start），仅用于排除上一轮
    /// 的速度，不进入 IPC payload。
    #[serde(skip)]
    pub round_started_at: Option<i64>,
    /// 最近一笔完成请求的 TTFT（time_to_first_token_ms，静态参考）；
    /// None → 前端显示 "–"
    pub ttft_ms: Option<i64>,
}

/// 今日（自然日本地零点起）全库 model_usage 合计（窗口级汇总行）。
/// total 与主面板 today 口径一致：Σcomputed_total_tokens（列缺失时
/// 等价退化为 input+output——实测 computed = input+output）。全 0 =
/// 今日无请求（前端不显示今日行）。
#[derive(Debug, Clone, Serialize, PartialEq, Default)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HudTodayTotal {
    /// Σ input_tokens（原值，含缓存读）
    #[serde(rename = "in")]
    pub in_tokens: i64,
    /// Σ output_tokens
    #[serde(rename = "out")]
    pub out_tokens: i64,
    /// Σ cache_read_input_tokens
    pub cache_read: i64,
    /// Σ computed_total_tokens（缺列时 input+output）
    pub total: i64,
    /// 请求笔数（model_usage 行数）
    pub req_count: i64,
}

/// 模型速度区单行（列表下方、今日行之上的按模型分组速度摘要）。
/// camelCase 与前端契约一致；口径见 collect_model_speeds。
#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub(crate) struct HudModelSpeed {
    /// 模型 id（原值展示；前端 truncate + title 全名）
    pub model: String,
    /// 最近一笔完成请求的输出速度 t/s（output ÷ 生成毫秒 × 1000，
    /// 实时感优先）
    pub tps: f64,
    /// 窗口内全部可信样本的总 output ÷ 总生成耗时（t/s，窗口均值）
    pub avg_tps: f64,
    /// 窗口内可信样本的单笔最快速度 t/s（与 avg_tps 同样本池）
    pub max_tps: f64,
    /// 窗口内可信样本的单笔最慢速度 t/s（与 avg_tps 同样本池）
    pub min_tps: f64,
    /// 可信速度样本数（completed 且 first/completed 时刻齐全时序正常
    /// 且 output > 0 的行数）
    pub samples: i64,
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
    /// 今日合计（自然日本地零点起全库 model_usage；窗口级汇总行，
    /// 随 show_tokens 配置隐藏——前端与窗口高度判定同条件，见
    /// today_visible；全 0 = 今日无请求，前端不显示该行）
    pub today_total: HudTodayTotal,
    /// 模型速度区（活跃窗口内按模型分组的速度摘要，按最近使用降序至多
    /// HUD_MAX_MODEL_ROWS 个；查询失败/老库缺列降级为空数组 → 前端不
    /// 渲染该区，窗口高度判定同条件见 model_speed_visible）
    pub model_speeds: Vec<HudModelSpeed>,
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
                today_total: HudTodayTotal::default(),
                model_speeds: Vec::new(),
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
    // 会话 → 最新 user 消息（生成中判定 + 新轮可靠身份）。仅尾部扫查
    // 窗口内有效，与活跃判定一致；id 比单纯 generating 边沿更可靠，能
    // 区分两轮之间未被轮询到空闲状态的快速连续发问。
    let mut msg_signals: BTreeMap<String, (String, i64)> = BTreeMap::new();
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
                "SELECT id, session_id, time_created, \
                        json_extract(data, '$.role') AS role \
                 FROM (SELECT id, session_id, time_created, data \
                       FROM message ORDER BY rowid DESC LIMIT ?1) \
                 WHERE time_created >= ?2 ORDER BY time_created DESC, id DESC",
            )
            .map_err(|e| format!("准备消息活跃查询失败: {e}"))?;
        let rows = stmt
            .query_map(
                rusqlite::params![MESSAGE_TAIL_ROWS, window_start],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, Option<i64>>(2)?,
                        row.get::<_, Option<String>>(3)?,
                    ))
                },
            )
            .map_err(|e| format!("读取消息活跃失败: {e}"))?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| format!("读取消息活跃失败: {e}"))?;
        for (message_id, session_id, at, role) in rows {
            let Some(at) = at else { continue };
            merge_activity(&mut activity, session_id.clone(), at);
            if role.as_deref() == Some("user") && at > 0 && !message_id.is_empty() {
                let replace = msg_signals
                    .get(&session_id)
                    .is_none_or(|(old_id, old_at)| at > *old_at || (at == *old_at && message_id > *old_id));
                if replace {
                    msg_signals.insert(session_id, (message_id, at));
                }
            }
        }
    }

    // 2) 会话树归并：共享索引负责任意深度后代、老库降级和环路保护。
    //    directory 只是展示元信息，缺失时不应让 parent_id 树能力失效。
    let tree_index = crate::token_speed::load_session_tree_index(conn)?;
    let has_directory_col = has_table(conn, "session")
        && crate::db::has_column(conn, "session", "id")
        && crate::db::has_column(conn, "session", "directory");

    let mut candidates: Vec<(String, i64)> = activity
        .iter()
        .map(|(id, at)| (id.clone(), *at))
        .collect();
    // 按最近活动倒序（正在生成的会话天然在最顶部）
    candidates.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));

    // 主会话槽位：先记一个非空目录回退值与归并后的最近活动时刻。
    // 最终目录会再次按 root 查询；这样子代理即使更晚/更活跃，也不能
    // 覆盖主会话自己的 directory。
    let mut mains: BTreeMap<String, (Option<String>, i64)> = BTreeMap::new();
    for (id, last_at) in &candidates {
        let root = tree_index.root_for(id);
        let directory = if has_directory_col {
            session_directory(conn, id)?
        } else {
            None
        };
        let slot = mains.entry(root).or_insert((None, 0));
        if slot.0.is_none() {
            slot.0 = usable_directory(directory);
        }
        if *last_at > slot.1 {
            slot.1 = *last_at;
        }
    }

    let mut ordered: Vec<(String, Option<String>, i64)> = Vec::with_capacity(mains.len());
    for (id, (fallback_dir, at)) in mains {
        // Root directory wins. A root can be absent from `candidates` when
        // only a child has recent activity, hence this independent lookup
        // rather than relying on the candidate loop above.
        let root_dir = if has_directory_col {
            usable_directory(session_directory(conn, &id)?)
        } else {
            None
        };
        ordered.push((id, root_dir.or(fallback_dir), at));
    }
    ordered.sort_by(|a, b| b.2.cmp(&a.2).then_with(|| a.0.cmp(&b.0)));

    // 3) 只对将展示的条目做会话树聚合；成员和徽标覆盖任意深度后代。
    let total_active = ordered.len();
    let mut sessions: Vec<HudSessionBrief> = Vec::new();
    let mut trees: Vec<SessionTree> = Vec::new();
    for (session_id, directory, last_at) in ordered {
        if sessions.len() >= MAX_VISIBLE_SESSIONS {
            continue;
        }
        let tree = tree_index.tree(&session_id);
        let members = tree.members.clone();
        let active_subs = members
            .iter()
            .filter(|member| *member != &session_id && activity.contains_key(*member))
            .count();
        // 树口径的最新 user 消息时刻（生成中判定的 message 驱动信号）
        let tree_last_msg = members
            .iter()
            .filter_map(|m| msg_signals.get(m))
            .max_by(|a, b| a.1.cmp(&b.1).then_with(|| a.0.cmp(&b.0)))
            .cloned();
        trees.push(tree);
        sessions.push(collect_session_brief(
            conn,
            &members,
            &session_id,
            window_start,
            directory.as_deref(),
            last_at,
            active_subs,
            tree_last_msg.as_ref(),
            now_ms,
        )?);
    }

    Ok((
        HudSnapshot {
            v: 1,
            total_active,
            sessions,
            today_total: collect_today_total(conn, today_start_ms(now_ms))?,
            model_speeds: collect_model_speeds(conn, window_start),
        },
        trees,
    ))
}

/// 本地时区自然日零点（毫秒，纯函数供测试）：与主面板 today /
/// lib.rs today_tray_title 的零点口径一致（不能用 UTC——东八区午前 UTC
/// 零点会把昨天 8 小时计入"今日"）。零点解析异常（DST 切换窄边缘）回
/// 退 now − 24h。
fn today_start_ms(now_ms: i64) -> i64 {
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

/// 今日合计（自然日本地零点起全库 model_usage）：与主面板 today /
/// 菜单栏 today_tray_title 同口径（全库不分会话树，input 用原值）。
/// total = Σcomputed_total_tokens（实测 computed = input+output；列
/// 缺失的老库等价退化为 input+output）。注意数据源不同：本函数直读
/// 主库 model_usage 实时值，today_tray_title 走派生库（30 秒节流导
/// 入），公式与本地零点口径一致但数值允许最长约 30 秒的瞬时差异。
/// 调用方已守卫核心列存在。
fn collect_today_total(conn: &Connection, today_start: i64) -> Result<HudTodayTotal, String> {
    let has = |col: &str| crate::db::has_column(conn, "model_usage", col);
    let num = |ok: bool, col: &str| {
        if ok {
            format!("COALESCE(SUM({col}), 0)")
        } else {
            "0".to_string()
        }
    };
    // total：优先 computed_total_tokens（与主面板 total_tokens 完全一致），
    // 缺列时由 input+output 等价合成（分项仍需逐列读取）
    let total_expr = if has("computed_total_tokens") {
        "COALESCE(SUM(computed_total_tokens), 0)".to_string()
    } else {
        format!(
            "{} + {}",
            num(has("input_tokens"), "input_tokens"),
            num(has("output_tokens"), "output_tokens")
        )
    };
    let sql = format!(
        "SELECT {}, {}, {}, {}, COUNT(*) \
         FROM model_usage WHERE started_at >= ?1",
        num(has("input_tokens"), "input_tokens"),
        num(has("output_tokens"), "output_tokens"),
        num(has("cache_read_input_tokens"), "cache_read_input_tokens"),
        total_expr,
    );
    conn.query_row(&sql, [today_start], |row| {
        Ok(HudTodayTotal {
            in_tokens: row.get::<_, i64>(0)?.max(0),
            out_tokens: row.get::<_, i64>(1)?.max(0),
            cache_read: row.get::<_, i64>(2)?.max(0),
            total: row.get::<_, i64>(3)?.max(0),
            req_count: row.get::<_, i64>(4)?.max(0),
        })
    })
    .map_err(|e| format!("读取今日合计失败: {e}"))
}

/// 模型速度区数据源（活跃窗口内按 model_id 分组的速度摘要）：窗口内
/// status='completed' 且 first_token_at/completed_at 齐全时序正常的
/// model_usage 行（turn_id NULL 的后台请求行不过滤——速度语义与轮次
/// 无关），每模型：
/// - tps = 最近一笔可信样本的 output ÷ (completed_at − first_token_at)
///   × 1000（最近一笔优先，实时感）；
/// - avg_tps = 窗口内全部可信样本的总 output ÷ 总生成耗时（窗口均值）；
/// - max_tps / min_tps = 同一可信样本池内的单笔最快 / 最慢速度（单笔样本
///   时三者同值）；
/// - samples = 可信样本数。
/// output ≤ 0 与除零（completed ≤ first、时刻 NULL/非正）的行跳过不
/// 计样本；status 列缺失的老库按全完成降级（与 collect_session_brief
/// 的完成判定同口径）。按最近使用时间降序，至多 HUD_MAX_MODEL_ROWS 个
/// 模型。任何查询失败/表或列缺失返回空数组（不阻塞快照——速度区是
/// 锦上添花信息，失败静默降级，下个轮询周期自然重试）。扫查行数以
/// MODEL_SPEED_SCAN_ROWS 为硬上限（started_at 降序 LIMIT，走
/// model_usage_started_model_idx 前缀索引，绝不全表扫）。
fn collect_model_speeds(conn: &Connection, window_start: i64) -> Vec<HudModelSpeed> {
    let has = |col: &str| crate::db::has_column(conn, "model_usage", col);
    if !has_table(conn, "model_usage") || !has("model_id") || !has("started_at") {
        return Vec::new();
    }
    let has_status = has("status");
    let status_expr = if has_status { "status" } else { "NULL" };
    let sql = format!(
        "SELECT COALESCE(model_id, ''), {status_expr}, started_at, {}, {}, {} \
         FROM model_usage \
         WHERE started_at >= ?1 AND model_id IS NOT NULL AND model_id != '' \
         ORDER BY started_at DESC LIMIT ?2",
        opt_expr(has("first_token_at"), "first_token_at"),
        opt_expr(has("completed_at"), "completed_at"),
        num_expr(has("output_tokens"), "output_tokens"),
    );
    let Ok(mut stmt) = conn.prepare(&sql) else {
        return Vec::new();
    };
    let rows = stmt.query_map(
        rusqlite::params![window_start, MODEL_SPEED_SCAN_ROWS],
        |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, Option<String>>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, Option<i64>>(3)?,
                row.get::<_, Option<i64>>(4)?,
                row.get::<_, i64>(5)?,
            ))
        },
    );
    let Ok(rows) = rows else {
        return Vec::new();
    };
    let observed_at_ms = chrono::Utc::now().timestamp_millis();

    // 单趟聚合：行按 started_at 降序，每模型首条可信样本即"最近一笔"。
    // 输出排序键 last_at 随聚合暂存，出口重排。
    struct ModelSpeedAgg {
        last_at: i64,
        last_tps: f64,
        /// 样本池内单笔最快 / 最慢速度（首笔样本在 or_insert 初始化）
        max_tps: f64,
        min_tps: f64,
        sum_out: i64,
        sum_ms: i64,
        samples: i64,
    }
    let mut aggs: BTreeMap<String, ModelSpeedAgg> = BTreeMap::new();
    for row in rows.flatten() {
        let (model, status, started_at, first, completed, output) = row;
        if model.is_empty() || started_at <= 0 {
            continue;
        }
        // 完成判定：status 列缺失按全完成降级；列存在时行值为 NULL/
        // error/cancelled 均不计（与 collect_session_brief 同口径）
        if has_status && !is_completed_status(status.as_deref()) {
            continue;
        }
        let Some(snapshot) = request_speed_at(
            &RequestTiming {
                output_tokens: output,
                started_at: Some(started_at),
                first_token_at: first,
                completed_at: completed,
                ..RequestTiming::default()
            },
            observed_at_ms,
        ) else {
            continue;
        };
        let Some((first, completed)) = first.zip(completed) else {
            continue;
        };
        let Some(gen_ms) = completed.checked_sub(first).filter(|value| *value > 0) else {
            continue;
        };
        let tps = snapshot.value;
        let entry = aggs.entry(model).or_insert(ModelSpeedAgg {
            last_at: started_at,
            last_tps: tps,
            max_tps: tps,
            min_tps: tps,
            sum_out: 0,
            sum_ms: 0,
            samples: 0,
        });
        // DESC 序首见即最近一笔；同刻多行取先到者（防御乱序 only >）
        if started_at > entry.last_at {
            entry.last_at = started_at;
            entry.last_tps = tps;
        }
        // 单笔最快 / 最慢（首笔已在 or_insert 初始化，逐笔比较即可）
        if tps > entry.max_tps {
            entry.max_tps = tps;
        }
        if tps < entry.min_tps {
            entry.min_tps = tps;
        }
        entry.sum_out += output;
        entry.sum_ms += gen_ms;
        entry.samples += 1;
    }

    let mut out: Vec<(i64, HudModelSpeed)> = aggs
        .into_iter()
        .filter(|(_, a)| a.sum_ms > 0)
        .map(|(model, a)| {
            (
                a.last_at,
                HudModelSpeed {
                    model,
                    tps: a.last_tps,
                    avg_tps: a.sum_out as f64 * 1000.0 / a.sum_ms as f64,
                    max_tps: a.max_tps,
                    min_tps: a.min_tps,
                    samples: a.samples,
                },
            )
        })
        .collect();
    // 按最近使用时间降序（同刻按模型名稳定排序），至多前 3 个模型
    out.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.model.cmp(&b.1.model)));
    out.truncate(HUD_MAX_MODEL_ROWS);
    out.into_iter().map(|(_, s)| s).collect()
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

/// 会话元信息：项目目录（directory 最后一段在调用方解析，此处回传原值）。
/// parent_id 由共享 SessionTreeIndex 读取，避免目录列缺失时树能力一起降级。
fn session_directory(conn: &Connection, session_id: &str) -> Result<Option<String>, String> {
    match conn.query_row(
        "SELECT directory FROM session WHERE id = ?1",
        [session_id],
        |row| row.get::<_, Option<String>>(0),
    ) {
        Ok(directory) => Ok(directory),
        Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
        Err(e) => Err(format!("读取会话信息失败: {e}")),
    }
}

fn usable_directory(directory: Option<String>) -> Option<String> {
    directory.filter(|value| !value.trim().is_empty())
}

/// 会话树（主会话 + 全部子代理）摘要：model_usage 按成员 IN 等值查
///（model_usage_session_turn_idx 前缀索引，每成员一次索引定位）单趟
/// 扫查完成全部统计。合计口径对齐注入版会话条：↑ = Σ max(0, input −
/// cache_read)，↓ = Σ output，⟲ = Σ cache_read，Σ = ↑+↓+⟲，× = 行数
///（每行一笔请求）；子代理并入主会话行、永不单独成行（V9 防双计）。
/// 另产出：模型集合（时间窗内 distinct，按最近使用降序）、生成中判定
///（树内任一成员最新轮未完成即生成中）、请求速度/TTFT（最近一笔完成
/// 请求；无有效请求级时间数据时为空值，生成状态由 speed_state 单独表达）。
fn collect_session_brief(
    conn: &Connection,
    members: &[String],
    session_id: &str,
    window_start: i64,
    directory: Option<&str>,
    last_active_at: i64,
    active_subs: usize,
    tree_last_msg: Option<&(String, i64)>,
    now_ms: i64,
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
    let first_expr = opt_expr(has("first_token_at"), "first_token_at");
    let completed_expr = opt_expr(has("completed_at"), "completed_at");

    // 树成员 IN 等值查：占位数与成员数一致（成员 ≤ 1 + 子代理数，
    // 个位数量级）
    let placeholders = vec!["?"; members.len()].join(", ");
    let sql = format!(
        "SELECT session_id, {in_expr}, {out_expr}, {cr_expr}, \
                COALESCE(model_id, ''), {turn_expr}, {status_expr}, started_at, \
                {dur_expr}, {ttft_expr}, {first_expr}, {completed_expr} \
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
    // 最新一笔跟踪（生成中判定）+ 完成请求候选（速度/TTFT）
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
        let completed_turn = turn.clone();
        let status: Option<String> = take_text(6)?;
        let started_at: Option<i64> = opt_cell(7)?;
        let started_ms = started_at.unwrap_or(0);
        let dur: Option<i64> = opt_cell(8)?;
        let ttft: Option<i64> = opt_cell(9)?;
        let first_token_at: Option<i64> = opt_cell(10)?;
        let completed_at: Option<i64> = opt_cell(11)?;

        // 合计（子代理并入主会话行；防双计靠"子代理永不单独成行"）
        agg.plain_in += (input - cache_read).max(0);
        agg.out_tokens += output.max(0);
        agg.cache_read += cache_read.max(0);
        agg.req_count += 1;

        // 模型集合：时间窗内 distinct，记录各模型最近使用时刻（排序用）
        if started_at.is_some_and(|value| value >= window_start) && !model.is_empty() {
            agg.models
                .entry(model.clone())
                .and_modify(|t| {
                    if started_ms > *t {
                        *t = started_ms;
                    }
                })
                .or_insert(started_ms);
        }

        // 最近一笔完成请求判定（速度/TTFT 数据源；status 列缺失按全完成
        // 降级——老库 model_usage 完成即落行，无 running 行）。须在
        // turn/status 被 tail 跟踪移动之前取值
        let is_completed = !has_status || is_completed_status(status.as_deref());

        // 成员最新一笔（生成中判定按成员跟踪，树内取或）。守卫分支保证
        // turn/status 的移动在各自分支内无条件发生（条件内移动会触发
        // maybe-moved 报错）
        if started_ms > 0 {
            match agg.tails.get_mut(&member) {
                Some(tail) if started_ms >= tail.latest_at => {
                    tail.latest_at = started_ms;
                    tail.turn = turn;
                    tail.status = status;
                }
                Some(_) => {}
                None => {
                    agg.tails.insert(
                        member,
                        MemberTail {
                            latest_at: started_ms,
                            turn,
                            status,
                        },
                    );
                }
            }
        }

        let request_order = completed_at
            .filter(|value| *value > 0)
            .or_else(|| {
                started_at.and_then(|started| {
                    started.checked_add(dur.filter(|value| *value > 0).unwrap_or(0))
                })
            })
            .unwrap_or(started_ms);
        // Keep the newest non-terminal/failed row as an invalid candidate so
        // an older completed request cannot remain visible while a newer
        // request in the same round is running or has been cancelled.
        let derived_ttft = is_completed.then(|| {
            ttft.or_else(|| {
                first_token_at
                    .zip(started_at)
                    .filter(|(first, started)| *first > *started)
                    .map(|(first, started)| first - started)
            })
        });
        agg.completed_candidates.push((
            request_order,
            LatestCompleted {
                turn_id: completed_turn,
                timing: RequestTiming {
                    output_tokens: if is_completed { output.max(0) } else { 0 },
                    started_at: started_at.filter(|value| *value > 0),
                    first_token_at,
                    completed_at,
                    duration_ms: dur,
                    request_id: None,
                },
                ttft: derived_ttft.flatten(),
            },
        ));
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
    // A1 新鲜期：消息只在 PENDING_FRESH_MS 内独立支撑"等待首请求"——
    // 超期的孤儿消息（真实主库观察到约 12 小时未匹配）不再永久冒充活
    // 跃轮；耗时很长的首请求若已落库首行请求，则由上方 tails 的未完成
    // 轮口径接管（可复核活跃证据），不受新鲜期影响。
    let tree_latest_req = agg.tails.values().map(|t| t.latest_at).max().unwrap_or(0);
    let msg_generating = tree_last_msg
        .map(|(_, at)| {
            *at > tree_latest_req && now_ms.saturating_sub(*at) <= PENDING_FRESH_MS
        })
        .unwrap_or(false);
    if msg_generating {
        generating = true;
    }

    // New-round identity is independent of the generating edge. A user
    // message id survives a fast consecutive question even when no poll sees
    // idle; if that signal is unavailable, a new model turn id is the safe
    // fallback. The timestamp is used only as a lower bound for eligible
    // completed speeds.
    // A1：超期孤儿消息（晚于全部请求、超出新鲜期且无活跃佐证）既不驱动
    // 生成态，也不再作为轮边界——否则它会永久排除此前所有请求的速度，
    // 让空闲会话显示 Unavailable。此时回退最新模型轮的 turn 边界。
    let boundary_msg = tree_last_msg.filter(|(_, at)| {
        *at <= tree_latest_req || now_ms.saturating_sub(*at) <= PENDING_FRESH_MS
    });
    let latest_turn = agg
        .tails
        .values()
        .filter_map(|tail| tail.turn.as_deref().filter(|turn| !turn.is_empty()).map(|turn| (turn, tail.latest_at)))
        .max_by_key(|(_, at)| *at);
    let latest_turn = latest_turn.map(|(turn, at)| (turn.to_string(), at));
    let (round_key, round_started_at) = if let Some((message_id, at)) = boundary_msg {
        (Some(format!("message:{message_id}")), (*at > 0).then_some(*at))
    } else if let Some((turn, at)) = latest_turn.as_ref() {
        (Some(format!("turn:{turn}")), (*at > 0).then_some(*at))
    } else {
        (None, None)
    };

    // A new user message is a round boundary, not merely a lower bound on
    // completion time: an older request can finish after that message while
    // its model_usage row is still the newest DB row. Exclude such a row by
    // its request start. Without a message signal, the latest model turn id
    // is the fallback boundary. This keeps a cancelled/empty new round from
    // inheriting the previous round's DB speed as well as its rollout speed.
    let latest_completed = agg
        .completed_candidates
        .into_iter()
        .filter(|(_, completed)| {
            if let Some(round_start) = round_started_at {
                completed
                    .timing
                    .started_at
                    .is_some_and(|started| started >= round_start)
            } else if let Some((turn, _)) = latest_turn.as_ref() {
                completed.turn_id.as_deref() == Some(turn.as_str())
            } else {
                true
            }
        })
        .max_by(|(left_order, _), (right_order, _)| left_order.cmp(right_order))
        .map(|(_, completed)| completed);

    // DB 口径速度：最新完成模型请求的可信生成速度，缺首字时间时才
    // 允许显式 request-average 近似。不能再用 turn_usage 总耗时估算。
    let lc = latest_completed.as_ref();
    let speed_snapshot = lc.and_then(|completed| request_speed_at(&completed.timing, now_ms));
    let speed = speed_snapshot.as_ref().map(|snapshot| snapshot.value);
    let speed_quality = speed_snapshot.as_ref().map(|snapshot| snapshot.quality);
    let speed_completed_at = speed_snapshot.as_ref().map(|snapshot| snapshot.completed_at);
    let lc_ttft = lc.and_then(|completed| completed.ttft);
    let speed_state = if generating {
        if speed.is_some() {
            SpeedState::Recent
        } else {
            SpeedState::Measuring
        }
    } else if speed.is_some() {
        SpeedState::Recent
    } else {
        SpeedState::Unavailable
    };

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
        speed_quality,
        speed_completed_at,
        speed_state,
        round_key,
        round_started_at,
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
    /// 请求候选（先按当前轮边界过滤，再选最近一笔）。未完成/失败的
    /// 最新行也作为无效候选保留，避免旧请求在新请求进行中继续污染速度；
    /// 同时避免旧请求在新用户消息之后才完成时污染新轮速度。
    completed_candidates: Vec<(i64, LatestCompleted)>,
}

/// 树内最近一笔请求摘要（速度 fallback / TTFT 共用数据源；无效候选的
/// output_tokens 为 0，因而只用于清空旧速度，不会被当作完成请求展示）。
#[derive(Clone)]
struct LatestCompleted {
    turn_id: Option<String>,
    timing: RequestTiming,
    ttft: Option<i64>,
}

/// 树内单个成员的最新一笔 model_usage 摘要（生成中判定输入）
#[derive(Default)]
struct MemberTail {
    latest_at: i64,
    turn: Option<String>,
    status: Option<String>,
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
// rollout 旁路速度源（增量读取 + 最近请求快照）
// ============================================================
//
// 数据事实（2026-09 本机实测）：~/.zcode/cli/rollout/model-io-
// {sessionId}.jsonl（sessionId 即 session 表 id），每行一次模型请求
// 完成即追加，JSON 含 usage.outputTokens（camelCase，注意与 DB 列名
// 不同）与 startedAt（RFC3339 毫秒）。文件由 ZCode 定期清理只留当日
// 活跃会话——HUD 只跟踪活跃会话恰好覆盖，文件缺失静默降级。
// 铁律：rollout 是旁路数据源，只服务速度展示，绝不并入 Σ/↑/↓/⟲/×
// 累计口径（累计以 db.sqlite model_usage 为唯一来源，防双计）。

/// rollout 单文件游标：偏移续读（参考 zcode_sessions 的 file_progress
/// 模式，仅存内存不落盘——文件由 ZCode 当日清理，HUD 只跟踪活跃会话）。
struct RolloutCursor {
    path: PathBuf,
    offset: u64,
}

/// 单会话树的速度跟踪器：文件游标 + 最新 rollout 请求 + 展示策略状态。
/// rollout 只提供尚未落入 DB 时的参考速度，不参与任何 token 累计。
struct TreeSpeed {
    root: String,
    cursors: Vec<RolloutCursor>,
    latest_rollout: Option<SpeedSnapshot>,
    latest_rollout_order: Option<i64>,
    /// 已看到的可靠 rollout request id，防止 truncate/重复读取导致重复
    /// 请求再次成为候选。没有可靠 id 时不猜测去重关系。
    seen_request_ids: BTreeSet<String>,
    ever_parsed: bool,
    /// 当前生成轮起始时刻；生成中只接受不早于该时刻的速度。
    round_started_at: Option<i64>,
    /// 用户消息/turn 身份；比 generating 边沿更可靠。没有身份时保留
    /// generating 边沿作为老库降级路径。
    round_key: Option<String>,
    last_generating: bool,
}

fn rollout_order(timing: &RequestTiming) -> Option<i64> {
    timing.completed_at.or_else(|| {
        timing
            .started_at
            .zip(timing.duration_ms)
            .and_then(|(started, duration)| started.checked_add(duration))
    }).or(timing.started_at)
}

impl TreeSpeed {
    /// 新树：每个成员一个游标；新游标从当前文件末尾起读，避免把历史
    /// 请求误当成当前可见会话刚出现时的新速度。
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
            latest_rollout: None,
            latest_rollout_order: None,
            seen_request_ids: BTreeSet::new(),
            ever_parsed: false,
            round_started_at: None,
            round_key: None,
            last_generating: false,
        }
    }

    /// 观察当前轮身份。身份变更时无论 generating 是否经过 false，都清除
    /// 上一轮 rollout 速度；取消后的 idle 快照仍带着当前身份，因此不会
    /// 把旧 rollout 重新显示出来。身份完全缺失时才退回 false→true 边沿。
    fn observe_round(
        &mut self,
        round_key: Option<&str>,
        started_at: Option<i64>,
        generating: bool,
    ) {
        let identity_changed = match (self.round_key.as_deref(), round_key) {
            (Some(current), Some(next)) => current != next,
            (None, Some(_)) => true,
            _ => false,
        };
        let fallback_changed = round_key.is_none() && generating && !self.last_generating;
        if identity_changed || fallback_changed {
            self.latest_rollout = None;
            self.latest_rollout_order = None;
            self.round_started_at = started_at.filter(|value| *value > 0);
        } else if self.round_started_at.is_none() {
            self.round_started_at = started_at.filter(|value| *value > 0);
        }
        if let Some(key) = round_key {
            self.round_key = Some(key.to_string());
        }
        self.last_generating = generating;
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

    /// 增量读取新完成请求并更新最新旁路快照：文件 stat 极轻，size 未变
    /// 跳过；size < offset 视为 truncate/重建归零重读；文件缺失静默
    /// 跳过；只消费完整行，末尾半行留待补完后续读；解析失败的行跳过
    ///（ZCode 私有格式无 schema 承诺）。
    fn ingest(&mut self, now_ms: i64) {
        // An older request can be appended after a newer user message while
        // the previous request is still finishing. Its completion time alone
        // is not enough to make it a current-round sample.
        let round_started_at = self.round_started_at;
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
                if let Some(request) = parse_rollout_line(line) {
                    if round_started_at.is_some_and(|started| {
                        request
                            .timing
                            .started_at
                            .is_none_or(|request_started| request_started < started)
                    }) {
                        continue;
                    }
                    if let Some(request_id) = request.timing.request_id.as_ref() {
                        if !self.seen_request_ids.insert(request_id.clone()) {
                            continue;
                        }
                    }
                    // The append observation time is never used as a synthetic
                    // completion time. The rollout record must carry either a
                    // real completedAt or a trusted durationMs.
                    self.ever_parsed = true;
                    let order = rollout_order(&request.timing);
                    let replace = match (self.latest_rollout_order, order) {
                        (None, _) => true,
                        (Some(current), Some(next)) => next >= current,
                        (Some(_), None) => false,
                    };
                    if replace {
                        self.latest_rollout_order = order;
                        self.latest_rollout = request_speed_at(&request.timing, now_ms);
                    }
                }
            }
            cursor.offset += consumed as u64 + 1;
        }
    }

    /// DB 与 rollout 的择优：DB 是权威来源；rollout 可以在 DB 尚未可见
    /// 时提供较新的参考。当前表面没有能可靠映射两者的一对一请求 ID，
    /// 因此不声称按 ID 合并：只按有效 completion timestamp 比较；相同
    /// 时刻 DB 胜出，rollout 严格更晚才胜出。并发请求若时间相同不会被
    /// 盲目合并，若时间接近但不相同则按时间新旧选择，不做时间窗猜测。
    /// 生成中新轮只接受本轮之后的请求。
    fn display_speed(
        &mut self,
        _generating: bool,
        fallback: Option<SpeedSnapshot>,
        _now_ms: i64,
    ) -> Option<SpeedSnapshot> {
        let eligible = |speed: &SpeedSnapshot| {
            // Once a reliable round identity is known, keep the lower bound
            // after cancellation/idle too. Restricting it to `generating`
            // would let the old rollout value reappear on the first idle poll.
            self.round_started_at
                .is_none_or(|started| speed.completed_at >= started)
        };
        let db = fallback.filter(|speed| eligible(speed));
        let rollout = self.latest_rollout.as_ref().filter(|speed| eligible(speed));
        // A newer rollout row can be invalid (zero output, reversed/future
        // timestamps, etc.). It still describes the newest request, so an
        // older DB value must not survive as if it were current. Equal
        // completion times remain a DB win; this is a timestamp tie-break,
        // not proof that the two rows are the same request.
        if self.latest_rollout.is_none()
            && self
                .latest_rollout_order
                .zip(db.as_ref().map(|speed| speed.completed_at))
                .is_some_and(|(rollout_order, db_completed)| rollout_order > db_completed)
        {
            return None;
        }
        match (db, rollout) {
            (Some(db), Some(rollout)) if rollout.completed_at > db.completed_at => {
                Some(rollout.clone())
            }
            (Some(db), _) => Some(db),
            (None, Some(rollout)) => Some(rollout.clone()),
            (None, None) => None,
        }
    }
}

/// rollout 旁路速度监控器（feed 线程私有状态）：按会话树跟踪文件游标
/// 与最近请求快照。
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

#[derive(Debug, Clone)]
struct RolloutRequest {
    timing: RequestTiming,
}

/// Parse one rollout line into request-level timing. The file is a private
/// JSONL surface and has changed shape across ZCode versions, so timestamps
/// accept either RFC3339 strings or millisecond numbers. Missing fields remain
/// missing and are handled by the shared speed validator.
fn parse_rollout_line(line: &[u8]) -> Option<RolloutRequest> {
    let value: serde_json::Value = serde_json::from_slice(line).ok()?;
    let object = value.as_object()?;
    let started_at = timestamp_value(object.get("startedAt").or_else(|| object.get("started_at")))?;
    let response = object.get("response")?.as_object()?;
    let usage = response.get("usage")?.as_object()?;
    let output_tokens = usage
        .get("outputTokens")
        .or_else(|| usage.get("output_tokens"))
        .and_then(|value| value.as_i64())?;
    let first_token_at = timestamp_value(
        object
            .get("firstTokenAt")
            .or_else(|| object.get("first_token_at")),
    );
    let completed_at = timestamp_value(
        object
            .get("completedAt")
            .or_else(|| object.get("completed_at")),
    );
    let duration_ms = object
        .get("durationMs")
        .or_else(|| object.get("duration_ms"))
        .and_then(value_i64);
    let request_id = object
        .get("requestId")
        .or_else(|| object.get("request_id"))
        .or_else(|| object.get("id"))
        .and_then(|value| value.as_str())
        .map(str::to_string);

    let duration_ms = completed_at
        .zip(Some(started_at))
        .and_then(|(completed, started)| {
            completed
                .checked_sub(started)
                .filter(|duration| *duration > 0)
        })
        .or(duration_ms);
    Some(RolloutRequest {
        timing: RequestTiming {
            output_tokens,
            started_at: Some(started_at),
            first_token_at,
            completed_at,
            duration_ms,
            request_id,
        },
    })
}

fn value_i64(value: &serde_json::Value) -> Option<i64> {
    value
        .as_i64()
        .or_else(|| value.as_u64().and_then(|value| i64::try_from(value).ok()))
}

fn timestamp_value(value: Option<&serde_json::Value>) -> Option<i64> {
    let value = value?;
    value_i64(value).or_else(|| {
        value
            .as_str()
            .and_then(|text| chrono::DateTime::parse_from_rfc3339(text).ok())
            .map(|date| date.timestamp_millis())
    })
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
        assert_eq!(default.height, None, "从未拖拽 = 自适应高度模式");
        assert_eq!(default.font_scale, HUD_FONT_SCALE_DEFAULT);
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
            "\"height\"",
            "\"fontScale\"",
            "\"opacity\"",
            "\"windowMinutes\"",
            "\"showTokens\"",
            "\"showModel\"",
        ] {
            assert!(text.contains(key), "session-hud.json 缺少字段 {key}：{text}");
        }
        // 旧版配置文件兼容：改版前残留的 showContextBar 字段被 serde
        // 默认忽略、缺 height/fontScale 按默认补齐（自适应高度 + 不缩放），
        // 解析不失败（下次保存自然收敛到新字段集）
        fs::write(
            &path,
            r#"{"enabled":true,"showContextBar":false,"windowMinutes":30}"#,
        )
        .unwrap();
        let legacy = read_config_at(&path).unwrap();
        assert!(legacy.enabled);
        assert_eq!(legacy.window_minutes, 30);
        assert_eq!(legacy.show_tokens, true, "缺字段按默认补齐");
        assert_eq!(legacy.height, None, "旧配置无用户尺寸 → 自适应模式");
        assert_eq!(legacy.font_scale, HUD_FONT_SCALE_DEFAULT);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 配置_clamp收敛() {
        // 宽度/高度/透明度越界夹回合法域；档位归一到四个合法值
        let c = SessionHudConfig {
            width: 100.0,
            height: Some(50.0),
            opacity: 5.0,
            window_minutes: 7,
            ..SessionHudConfig::default()
        }
        .clamped();
        assert_eq!(c.width, HUD_MIN_WIDTH);
        assert_eq!(c.height, Some(HUD_MIN_HEIGHT));
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
        // NaN 防御回默认宽度；height NaN/负值回 None（自适应模式）
        let c = SessionHudConfig {
            width: f64::NAN,
            height: Some(f64::NAN),
            ..SessionHudConfig::default()
        }
        .clamped();
        assert_eq!(c.width, HUD_DEFAULT_WIDTH);
        assert_eq!(c.height, None);
        // 字体缩放：合法域内保留，越界/脏值回退 1.0
        assert_eq!(
            SessionHudConfig { font_scale: 1.25, ..SessionHudConfig::default() }
                .clamped()
                .font_scale,
            1.25
        );
        for dirty in [0.5, 2.0, f64::NAN, f64::INFINITY] {
            assert_eq!(
                SessionHudConfig { font_scale: dirty, ..SessionHudConfig::default() }
                    .clamped()
                    .font_scale,
                HUD_FONT_SCALE_DEFAULT,
                "字体缩放脏值 {dirty} 应回退默认"
            );
        }
        // 开关不被 clamp 改动
        let c = SessionHudConfig { enabled: true, ..SessionHudConfig::default() }.clamped();
        assert!(c.enabled);
    }

    #[test]
    fn 配置_局部更新只改目标字段并收敛() {
        // set_session_hud_font_scale / set_session_hud_opacity 的纯函数内核：
        // 悬浮窗内滑块只能改自己的字段，窗口尺寸/位置/开关/档位/显示项
        // 必须原样保留（否则拖一次滑块就把用户拖出的窗口尺寸或开关状态
        // 覆盖掉）
        let base = SessionHudConfig {
            enabled: true,
            pos: Some((12.0, 34.0)),
            width: 420.0,
            height: Some(300.0),
            font_scale: 1.2,
            opacity: 0.8,
            window_minutes: 30,
            show_tokens: false,
            show_model: false,
        };

        // 字体缩放：只动 font_scale，其余字段逐字段保留
        let font = apply_partial_update(base.clone(), |cfg| cfg.font_scale = 1.05);
        assert_eq!(font.font_scale, 1.05);
        assert_eq!(
            SessionHudConfig { font_scale: 1.05, ..base.clone() },
            font,
            "字体缩放局部更新不得改动其它字段"
        );

        // 透明度：滑块刻度 / 100 的 0.25~1.0 内直通
        let opacity = apply_partial_update(base.clone(), |cfg| cfg.opacity = 0.45);
        assert_eq!(opacity.opacity, 0.45);
        assert_eq!(
            SessionHudConfig { opacity: 0.45, ..base.clone() },
            opacity,
            "透明度局部更新不得改动其它字段"
        );

        // 越界值仍走 clamp：透明度夹回合法域、字体缩放脏值回退默认
        assert_eq!(
            apply_partial_update(base.clone(), |cfg| cfg.opacity = 5.0).opacity,
            HUD_OPACITY_RANGE.1
        );
        assert_eq!(
            apply_partial_update(base.clone(), |cfg| cfg.opacity = f64::NAN).opacity,
            HUD_OPACITY_DEFAULT
        );
        assert_eq!(
            apply_partial_update(base, |cfg| cfg.font_scale = 9.0).font_scale,
            HUD_FONT_SCALE_DEFAULT
        );
    }

    #[test]
    fn 窗口高度_会话条数自适应与空态下限() {
        // 栅格锁定：与 session-hud.html 的 CSS 常量一一对应
        //（头部 30 / 行 56 / 列表区边框 1 / 折叠行 20 / 今日行 20 /
        // 模型速度行 20 + 区边框 1 / 底部留白 8 / 空态 64）
        assert_eq!(HUD_ROW_H, 56.0);
        assert_eq!(HUD_TODAY_H, 20.0);
        assert_eq!(HUD_MODEL_ROW_H, 20.0);
        assert_eq!(HUD_LIST_BORDER_H, 1.0);
        assert_eq!(HUD_MODELS_BORDER_H, 1.0);
        // 空列表：空态最小高（显示"暂无活跃会话"，不为 0 高；空态不
        // 显示今日行与模型速度区——两者都跟随列表存在）
        assert_eq!(hud_height(0, false, false, 0, 1.0), HUD_EMPTY_HEIGHT);
        assert_eq!(hud_height(0, true, false, 0, 1.0), HUD_EMPTY_HEIGHT);
        assert_eq!(
            hud_height(0, false, true, 0, 1.0),
            HUD_EMPTY_HEIGHT,
            "空态不显示今日行"
        );
        assert_eq!(
            hud_height(0, false, false, 3, 1.0),
            HUD_EMPTY_HEIGHT,
            "空态不显示模型速度区"
        );
        // 头部 + 行 × 条数 + 列表区边框 + 底部留白
        assert_eq!(
            hud_height(1, false, false, 0, 1.0),
            30.0 + 56.0 + 1.0 + 8.0
        );
        assert_eq!(
            hud_height(3, false, false, 0, 1.0),
            30.0 + 3.0 * 56.0 + 1.0 + 8.0
        );
        // 超出上限折叠：5 行 + 折叠提示行 + 今日行
        assert_eq!(
            hud_height(9, true, true, 0, 1.0),
            30.0 + 5.0 * 56.0 + 1.0 + 20.0 + 20.0 + 8.0
        );
        // 折叠标志在可见行数内不生效
        assert_eq!(
            hud_height(2, true, false, 0, 1.0),
            hud_height(2, false, false, 0, 1.0) + 0.0
        );
        // 今日行独立叠加
        assert_eq!(
            hud_height(2, false, true, 0, 1.0),
            hud_height(2, false, false, 0, 1.0) + HUD_TODAY_H
        );
        // 模型速度区：每行 20px + 区边框 1px，超过 3 行按上限 3 行计
        assert_eq!(
            hud_height(2, false, false, 2, 1.0),
            hud_height(2, false, false, 0, 1.0) + 2.0 * HUD_MODEL_ROW_H + HUD_MODELS_BORDER_H
        );
        assert_eq!(
            hud_height(2, false, false, 5, 1.0),
            hud_height(2, false, false, 0, 1.0) + 3.0 * HUD_MODEL_ROW_H + HUD_MODELS_BORDER_H,
            "模型速度行数超过上限按 3 行计"
        );
        // 字体缩放：整体等比缩放（与页面 --hud-scale 栅格一致）
        assert_eq!(
            hud_height(2, false, false, 0, 1.4),
            hud_height(2, false, false, 0, 1.0) * 1.4
        );
        assert_eq!(
            hud_height(2, false, false, 0, 0.8),
            hud_height(2, false, false, 0, 1.0) * 0.8
        );
        // 脏 font_scale 回退 1.0
        assert_eq!(
            hud_height(2, false, false, 0, 3.0),
            hud_height(2, false, false, 0, 1.0)
        );
        assert_eq!(
            hud_height(2, false, false, 0, f64::NAN),
            hud_height(2, false, false, 0, 1.0)
        );
    }

    #[test]
    fn 程序侧目标尺寸_取整与最小高夹取() {
        // 取整：font_scale（0.8~1.4 步进 0.05）让 hud_height 出 .25 / .75
        // 等小数（95 × 1.05 = 99.75）——LogicalSize 小数到物理像素四舍五入
        // 后回声与目标精确相等判定不成立，会被 Resized 挂点当成用户尺寸
        // 落盘（自适应高度静默变固定尺寸），故目标先取整
        assert_eq!(hud_target_size(300.0, 217.35), (300.0, 217.0));
        assert_eq!(hud_target_size(300.0, 227.7), (300.0, 228.0));
        assert_eq!(hud_target_size(300.0, 258.75), (300.0, 259.0));
        // 与公式出口直连：3 行 × font_scale 1.05 = 217.35 → 217
        assert_eq!(
            hud_target_size(300.0, hud_height(3, false, false, 0, 1.05)),
            (300.0, 217.0)
        );
        assert_eq!(
            hud_target_size(300.0, hud_height(3, false, false, 0, 1.1)),
            (300.0, 228.0)
        );
        assert_eq!(
            hud_target_size(300.0, hud_height(5, false, false, 0, 1.05)),
            (300.0, 335.0)
        );
        // 最小高夹取：建窗 min_inner_size（HUD_MIN_HEIGHT = 160）会钳住更矮
        // 的 set_size（空态 64 / 单行 95 / 两行 151 全在其下，窗口实际就是
        // 160 高），目标不夹取则永远对不上回声（同上误判）
        assert_eq!(hud_target_size(300.0, 64.0), (300.0, 160.0));
        assert_eq!(
            hud_target_size(300.0, hud_height(0, false, false, 0, 1.0)),
            (300.0, 160.0),
            "空态 64 被 min_inner_size 钳到 160"
        );
        assert_eq!(
            hud_target_size(300.0, hud_height(1, false, false, 0, 1.0)),
            (300.0, 160.0),
            "单行 95 被 min_inner_size 钳到 160"
        );
        assert_eq!(
            hud_target_size(300.0, hud_height(2, false, false, 0, 1.0)),
            (300.0, 160.0),
            "两行 151 被 min_inner_size 钳到 160"
        );
        // 不低于最小高：自适应值本身即生效尺寸，原样保留
        assert_eq!(hud_target_size(300.0, 207.0), (300.0, 207.0));
        // 宽度同口径：取整（配置手改小数不产生残差）+ 夹回拖拽合法域
        assert_eq!(hud_target_size(283.3333333333333, 207.0), (283.0, 207.0));
        assert_eq!(hud_target_size(1120.5, 207.0), (1121.0, 207.0));
        assert_eq!(hud_target_size(120.0, 207.0), (HUD_MIN_WIDTH, 207.0));
    }

    #[test]
    fn 回声与目标一致_容差覆盖缩放取整残差() {
        // 精确相等（程序侧目标就是回声自身）：一致
        assert!(size_matches_target((300.0, 207.0), Some((300.0, 207.0))));
        // 系统缩放 125% / 150%：逻辑目标转物理像素取整后回除不复原原值
        //（1.25 × 98 = 122.5 → 123 → 98.4；1.5 × 99 = 148.5 → 149 →
        // 99.33），残差落在容差内——不认这类回声就会把程序自身的 set_size
        // 当成用户尺寸落盘（自适应高度静默冻结）
        assert!(size_matches_target((300.0, 98.4), Some((300.0, 98.0))));
        assert!(size_matches_target((300.0, 99.33333333333333), Some((300.0, 99.0))));
        assert!(size_matches_target((283.4, 207.2), Some((283.0, 207.0))));
        // 容差外：真·用户拖拽，需落盘
        assert!(!size_matches_target((300.0, 240.0), Some((300.0, 200.0))));
        assert!(!size_matches_target((420.0, 207.0), Some((300.0, 207.0))));
        // 任一轴超出容差即不一致（宽度拖出 30px 必须落盘）
        assert!(!size_matches_target((330.0, 207.2), Some((300.0, 207.0))));
        // 无目标记录（刚建窗 / 首拍）不算回声
        assert!(!size_matches_target((300.0, 207.0), None));
    }

    #[test]
    fn 今日行显隐_跟随数据行开关() {
        // 与前端 renderShell 的 showToday 同条件：show_tokens 关闭即整行
        // 隐藏（不想看数字的用户今日合计也不显示，窗口高度随之收缩）
        assert!(today_visible(true, 100, 2));
        assert!(!today_visible(false, 100, 2), "关闭数据行时今日行一并隐藏");
        assert!(!today_visible(true, 0, 2), "今日无请求不显示");
        assert!(!today_visible(true, 100, 0), "空态不显示今日行");
    }

    #[test]
    fn 模型速度区显隐_跟随数据行开关() {
        // 与前端 renderShell 的 showModels 逐字同条件：show_tokens 关闭、
        // 窗口内无可信样本或空态均不显示（速度属数字信息，与今日行同
        // 规则随数据行开关隐藏，窗口高度随之增减）
        assert!(model_speed_visible(true, 2, 3));
        assert!(
            !model_speed_visible(false, 2, 3),
            "关闭数据行时模型速度区一并隐藏"
        );
        assert!(!model_speed_visible(true, 0, 3), "无可信样本不显示");
        assert!(!model_speed_visible(true, 2, 0), "空态不显示模型速度区");
    }

    #[test]
    fn 尺寸同步_用户调整中跳过且同尺寸不重复设置() {
        // 尺寸变化 → 需同步；同尺寸 → 跳过（防无谓 set_size 回声）
        assert!(should_sync_size(false, Some((300.0, 200.0)), (305.0, 220.0)));
        assert!(!should_sync_size(false, Some((300.0, 200.0)), (300.0, 200.0)));
        // 无上次目标（首拍）→ 需同步
        assert!(should_sync_size(false, None, (300.0, 200.0)));
        // 用户正在拖拽热区（HUD_USER_RESIZING 置位）→ 一律跳过：否则自适应
        // 高度模式每拍把高度拉回公式值，用户拖不动 / 闪跳
        assert!(!should_sync_size(true, Some((300.0, 200.0)), (305.0, 400.0)));
        assert!(!should_sync_size(true, None, (305.0, 400.0)));
        // 标志置位 / 清除（前端 begin/end 命令 + 窗口销毁重建兜底共用）
        set_user_resizing(true);
        assert!(user_resizing());
        set_user_resizing(false);
        assert!(!user_resizing());
        // ---- Bug 1 顺序契约锁定：end 先落盘后清标志 ----
        // session_hud_resize_end 必须先完成终值落盘再清标志。下面用真实
        // 全局状态复现 end 的关键序列（置位 → 落盘判定 → 清理），锁定
        // "落盘完成前 poll_db 的 set_size 判定必须被标志拦住"：若 end 先
        // 清标志（旧 Bug），清标志后、cfg.height 落盘前的竞态窗口内
        // poll_db 读旧配置（height=None）算自适应高度并 set_size，把窗口
        // 打回自适应值（松手后高度弹回 / 闪跳）
        set_user_resizing(true);
        store_resize_snapshot((300.0, 207.0));
        // 落盘尚未完成（persist 之前）：poll_db 判定必须跳过（标志置位）
        assert!(
            !should_sync_size(user_resizing(), Some((300.0, 207.0)), (300.0, 400.0)),
            "落盘完成前 poll_db 不得放行 set_size（防打回自适应值）"
        );
        assert!(!should_apply_size_now(user_resizing()));
        // 收尾（persist 已完成，cfg.height 已是用户值）：清标志 + 清快照
        clear_resize_snapshot();
        set_user_resizing(false);
        // 清标志后放行：此时 cfg.height 已落盘为用户值，poll_db 的 set_size
        // 与用户值同值，冗余无害
        assert!(should_sync_size(user_resizing(), Some((300.0, 207.0)), (300.0, 400.0)));
    }

    #[test]
    fn 用户活动宽限期_纯函数时序判定() {
        // 从未有用户 Resized（锚点 0）：不拦截（与空闲态一致，宽限期不是
        // 常驻抑制）
        assert!(!user_size_active(false, 0, 1_000_000));
        // 标志置位恒为活动期（正常路径：begin 置位 → end 清除）
        assert!(user_size_active(true, 0, 1_000_000));
        // 最后用户 Resized 后不足宽限期：拦截（promise 提前 settle 后用户
        // 仍在拖的整段由每帧刷新的锚点持续覆盖；松手后仍在事件队列里的
        // 最后一个 Resized 事件同理）
        assert!(user_size_active(false, 10_000, 12_499));
        // 恰好到期（2.5 秒边界）：解除拦截
        assert!(!user_size_active(false, 10_000, 12_500));
        assert!(!user_size_active(false, 10_000, 60_000));
        // 锚点晚于当前时刻（时钟回拨 / 残值）：saturating_sub 归 0 判活动，
        // 不产生 underflow panic
        assert!(user_size_active(false, 20_000, 10_000));
    }

    #[test]
    fn 用户活动宽限期_锚点刷新驱动poll持续让路() {
        // 复现根因 A 时序并锁定拦截链路：N 向原生 startResizeDragging 的
        // promise 提前 settle（Windows 竞态）→ end 提前清标志 → 用户继续
        // 拖。期间每帧 Resized 经挂点刷新锚点（此处直接驱动
        // mark_user_resized 模拟挂点侧写），poll_db 的两处拦截判定都必须
        // 持续让路——否则 set_size 与拖拽打架（限高观感）+ 终值无人落盘
        mark_user_resized(0); // 复位（静态量全局共享，避免影响其它用例）
        // promise 提前 settle 后、下一帧 Resized 到达前：标志已清且锚点
        // 陈旧，不拦截（宽限期尚未被新帧锚定——挂点先于本判定刷新则自然
        // 进入拦截，见下）
        assert!(!user_size_active(false, last_user_resized_at(), 10_000));
        // 用户继续拖：每帧 Resized 刷新锚点（挂点侧写）
        mark_user_resized(10_500);
        assert_eq!(last_user_resized_at(), 10_500);
        // 宽限期内：should_sync_size（poll_db 主判定）与 should_apply_size_
        // now（闭包双检）都必须拦截
        assert!(
            !should_sync_size(
                user_size_active(false, last_user_resized_at(), 12_999),
                Some((300.0, 258.0)),
                (300.0, 400.0)
            ),
            "宽限期内 poll_db 不得放行 set_size（防打架 + 终值落盘）"
        );
        assert!(
            !should_apply_size_now(user_size_active(
                false,
                last_user_resized_at(),
                12_999
            )),
            "闭包双检同样让路（执行时刻重新合成判定）"
        );
        // 持续拖拽任意久都覆盖：每帧刷新锚点，宽限期从最后一帧起算
        mark_user_resized(600_000);
        assert!(user_size_active(false, last_user_resized_at(), 602_499));
        // 真实松手（最后一帧后 2.5s）：拦截解除，poll_db 恢复正常同步
        assert!(!user_size_active(false, last_user_resized_at(), 602_500));
        mark_user_resized(0); // 复位
    }

    #[test]
    fn 宽限期到期核校_纯函数触发判定() {
        // 宽限期内（用户仍可能在拖 / 终值尚未定格）：不核校——此时任何
        // 程序侧动作（含读尺寸落盘）都可能与拖拽打架或读到中间值
        assert!(!grace_check_due(true, true));
        assert!(!grace_check_due(true, false));
        // 宽限期刚过且有 pending（宽限期内出现过用户尺寸活动且尚未核校）：
        // 核校——终值兜底落盘的唯一触发形态
        assert!(grace_check_due(false, true));
        // 无 pending（从未有用户尺寸活动 / 上一轮已核校消费）：不核校，
        // 空闲态零额外开销（每拍只多一次原子读）
        assert!(!grace_check_due(false, false));
    }

    #[test]
    fn 宽限期待核校标志_置位与消费生命周期() {
        // 挂点置位（record_user_size 判定为用户尺寸时与 mark_user_resized
        // 同处）→ poll_db 到期核校消费（无论是否落盘、无论读取成败都必须
        // 消费——否则空闲期每拍重复核校）；置位可重复（拖拽每帧都置，幂等）
        clear_grace_pending();
        assert!(!grace_pending(), "初始/复位态无待核校");
        mark_grace_pending();
        assert!(grace_pending(), "用户尺寸事件置位待核校");
        mark_grace_pending();
        assert!(grace_pending(), "重复置位幂等（拖拽每帧）");
        clear_grace_pending();
        assert!(!grace_pending(), "核校完成必须消费 pending");
        clear_grace_pending(); // 复位收尾（静态量全局共享，避免影响其它用例）
    }

    #[test]
    fn 宽限期到期核校_终值丢失形态的触发时序() {
        // 复现修复目标的完整时序（根因兜底链路）：promise 提前 settle →
        // resize_end 提前执行（清标志/快照 + persist 当时值）→ 用户继续拖
        // 的最后一段距上次节流 persist < 1 秒、松手后没有第二次 end → 终值
        // 无人落盘 → 宽限期一过 poll_db 将按旧 cfg.height set_size 拉回。
        // 修复链路：拖拽期间每帧 Resized 都置 pending（挂点侧写：与
        // mark_user_resized 同处置位）→ 宽限期一过触发一次核校 → 消费后
        // 不再重复核校。核校本体的"读实际尺寸 + 兜底落盘 + 重读 cfg 覆盖
        // 局部变量"在 poll_db 内联执行（依赖 AppHandle 与主线程事件循环，
        // 不可单元测试），本用例锁定其触发时序契约
        mark_user_resized(0);
        clear_grace_pending();
        // 最后一帧用户 Resized（挂点侧写）
        mark_user_resized(10_000);
        mark_grace_pending();
        let active_at = |now: u64| user_size_active(false, last_user_resized_at(), now);
        // 宽限期内：不核校（等终值定格 / 不与拖拽打架）
        assert!(!grace_check_due(active_at(12_499), grace_pending()));
        // 宽限期刚过（12_500 边界）：核校触发——终值落盘的最后机会
        assert!(grace_check_due(active_at(12_500), grace_pending()));
        // 核校完成消费 pending：同拍后续 / 之后每拍都不再核校（与
        // resize_end 的正常核校路径幂等共存，persist 同值直接跳过写盘）
        clear_grace_pending();
        assert!(!grace_check_due(active_at(12_600), grace_pending()));
        mark_user_resized(0); // 复位
    }

    #[test]
    fn 尺寸纪元_递增作废feed局部记忆强制重同步() {
        // 复现根因 B 并锁定自愈链路：用户拖到 600，feed 线程局部 last_size
        // 仍停在记忆值 (300,258)——目标与记忆相等时 should_sync_size 跳过，
        // HWND 与程序认知脱钩（600 空壳 / WebView 视口停在 258）。resize_end
        // 收尾递增尺寸纪元（真实调用点在 session_hud_resize_end）→ feed
        // 线程作废局部记忆 → 下一拍必然放行一次 set_size（强制 HWND/WebView
        // 与配置一致）
        let mut last_size = Some((300.0, 258.0)); // feed 线程局部记忆（脱钩值）
        let mut base = size_epoch();
        let target = (300.0, 258.0); // cfg.height 未落盘时下一拍的自适应目标
        // 纪元未变：记忆 == 目标 → 跳过（脱钩态即 Bug 现状）
        assert!(!should_sync_size(false, last_size, target));
        // resize_end 收尾 / 建窗路径递增纪元
        bump_size_epoch();
        let now_epoch = size_epoch();
        assert_ne!(now_epoch, base, "递增必须可被 feed 线程观测");
        if now_epoch != base {
            base = now_epoch;
            last_size = None; // feed_loop 的纪元比对逻辑（见 HUD_SIZE_EPOCH）
        }
        assert_eq!(last_size, None, "纪元变化后局部记忆必须作废");
        // 下一拍：无记忆 → 必然放行一次 set_size（强制同步，消除脱钩态）
        assert!(should_sync_size(false, last_size, target));
        // 同步成功后记忆恢复，同目标不再重复 set_size（既有语义不变）
        last_size = Some(target);
        assert!(!should_sync_size(false, last_size, target));
    }

    #[test]
    fn 拖拽落盘轴向合并_纯拖宽不固化自适应高度() {
        // Bug 2 锁定：用户纯拖宽度（高度与拖前快照一致，容差内）时
        // cfg.height 不被当时的自适应高度覆写
        // 快照存在 + 高度未动（自适应模式）→ height 保持 None（自适应
        // 不被悄悄冻结，会话增多时高度继续自适应）
        assert_eq!(
            merge_persist_axes(None, (380.0, 207.0), Some((300.0, 207.0))),
            (380.0, None)
        );
        // 快照存在 + 高度未动（容差覆盖取整 / 缩放残差，原用户固定高度）
        // → height 保持原用户值
        assert_eq!(
            merge_persist_axes(Some(400.0), (380.0, 400.4), Some((300.0, 400.0))),
            (380.0, Some(400.0))
        );
        // 快照存在 + 高度变了（超出容差）→ 写死用户值
        assert_eq!(
            merge_persist_axes(None, (300.0, 400.0), Some((300.0, 207.0))),
            (300.0, Some(400.0))
        );
        // 快照缺失（begin 读取失败 / 飞快拖拽的窄竞态）→ 现状行为：两轴都写
        assert_eq!(
            merge_persist_axes(None, (380.0, 207.0), None),
            (380.0, Some(207.0))
        );
        assert_eq!(
            merge_persist_axes(Some(400.0), (380.0, 260.0), None),
            (380.0, Some(260.0))
        );
        // 宽度轴恒为拖后值（宽度是纯用户语义，无自适应，不受快照影响）
        assert_eq!(
            merge_persist_axes(None, (260.0, 207.0), Some((4096.0, 207.0))),
            (260.0, None)
        );
    }

    #[test]
    fn 拖前快照_存取与清理生命周期() {
        // begin 存入 / end 与兜底路径（建窗 / 销毁）清理：残留旧快照会
        // 污染下一次拖拽的高度轴判定（merge_persist_axes 拿旧值当拖前
        // 基准），生命周期必须闭环
        clear_resize_snapshot();
        assert_eq!(resize_snapshot(), None, "会话外无快照");
        store_resize_snapshot((300.0, 207.0));
        assert_eq!(resize_snapshot(), Some((300.0, 207.0)));
        clear_resize_snapshot();
        assert_eq!(resize_snapshot(), None, "end 收尾后快照必须清空");
    }

    #[test]
    fn 尺寸双检_投递闭包执行时复查标志() {
        // V2 竞态消除：poll_db 的 set_size 闭包从投递到主线程实际执行存在
        // 时间窗，期间用户可能按下热区——闭包内复查 user_resizing，置位则
        // 放弃本次 set_size（防排队闭包把拖拽起始阶段的高度拉回公式值）
        assert!(should_apply_size_now(false), "空闲时投递闭包正常 set_size");
        assert!(
            !should_apply_size_now(true),
            "闭包执行时用户已开始拖拽 → 跳过 set_size"
        );
    }

    #[test]
    fn 热区终值落盘判定_未拖动与脏尺寸不落盘() {
        // 前端拖出的尺寸与程序侧目标不同 → 落盘
        assert!(should_persist_resize_result((305.0, 240.0), Some((300.0, 200.0))));
        // 无目标记录（首拍 / 刚建窗）→ 落盘
        assert!(should_persist_resize_result((305.0, 240.0), None));
        // 点住热区未拖动（≈ 程序侧目标）：不落盘——自适应高度模式不被一次
        // 点击误冻结成用户固定尺寸；容差内（取整 / 缩放换算残差）同样不落
        assert!(!should_persist_resize_result((300.0, 200.0), Some((300.0, 200.0))));
        assert!(!should_persist_resize_result((300.4, 199.6), Some((300.0, 200.0))));
        // 低于最小尺寸（最小化 / 系统抖动）→ 不落盘（与 Resized 挂点同口径）
        assert!(!should_persist_resize_result((120.0, 100.0), Some((300.0, 200.0))));
        assert!(!should_persist_resize_result((300.0, 100.0), Some((300.0, 200.0))));
    }

    #[test]
    fn 尺寸槽_回声不进槽用户尺寸进槽() {
        // 槽（HUD_SIZE）只承载"用户产生的尺寸"：Destroyed 挂点的关窗冲刷
        // 无节流直接读槽落盘，程序侧 set_size 回声一旦进槽，关窗时就会被
        // 当成用户尺寸写进 cfg.height，自适应高度模式被永久冻结
        let read_slot = || *hud_size_slot().lock().unwrap();
        *hud_size_slot().lock().unwrap() = None;
        // 程序侧自适应 set_size 的回声（与 LAST_SIZE 一致）：不进槽
        assert!(!record_user_size((300.0, 207.0), Some((300.0, 207.0))));
        assert_eq!(read_slot(), None, "回声不写内存槽（关窗冲刷不落盘）");
        // 缩放取整残差回声（125% 下 1.25 × 98 = 122.5 → 123 → 98.4）同理
        assert!(!record_user_size((300.0, 98.4), Some((300.0, 98.0))));
        assert_eq!(read_slot(), None);
        // 低于最小尺寸（最小化 / 系统抖动）：不进槽
        assert!(!record_user_size((120.0, 100.0), Some((300.0, 207.0))));
        assert_eq!(read_slot(), None);
        // 用户拖拽出的尺寸（与程序侧目标不同）：进槽 → 关窗冲刷落盘的就是它
        assert!(record_user_size((380.0, 260.0), Some((300.0, 207.0))));
        assert_eq!(read_slot(), Some((380.0, 260.0)));
        // 用户尺寸之后再收到程序回声：不覆盖槽里已有的用户尺寸（关窗仍落
        // 用户值，不会被随后一拍自适应 set_size 冲掉）
        assert!(!record_user_size((300.0, 207.0), Some((300.0, 207.0))));
        assert_eq!(read_slot(), Some((380.0, 260.0)));
        // 无目标记录（刚建窗 / 首拍）按用户尺寸处理
        assert!(record_user_size((380.0, 260.0), None));
        assert_eq!(read_slot(), Some((380.0, 260.0)));
        // 复位（静态槽全局共享，避免影响其它用例）
        *hud_size_slot().lock().unwrap() = None;
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
    fn 快照_主会话directory优先于子代理目录() {
        let (conn, path) = hud_db("root-directory");
        let now = 1_100_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES
               ('sess_root', NULL, '/workspace/main-project'),
               ('sess_child', 'sess_root', '/workspace/wrong-child-project');
             INSERT INTO model_usage VALUES
               ('sess_child', 'turn_child', {at}, 'M', 'completed', 10, 20, 0, 0, 0, 30);",
            at = now - 100,
        ))
        .unwrap();
        let snapshot = collect_session_snapshot(&conn, 10, now).unwrap().0;
        assert_eq!(snapshot.sessions.len(), 1, "子代理只能归并到根会话");
        assert_eq!(snapshot.sessions[0].session_id, "sess_root");
        assert_eq!(snapshot.sessions[0].project, "main-project");
        drop(conn);
        let _ = fs::remove_file(&path);
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
               ('sess_sub1', 'sess_aaa111', '/Users/a/proj-alpha'),
               ('sess_sub1_child', 'sess_sub1', '/Users/a/proj-alpha');
             INSERT INTO model_usage VALUES
               -- 会话 A：窗口内 1 秒前有请求（最新，应排第一）
               ('sess_aaa111', 'turn_a', {ta}, 'GLM-5.3', 'completed', 100, 200, 0, 10, 50, 300),
               -- 会话 A 早期请求（计入全生命周期累计）
               ('sess_aaa111', 'turn_a0', {ta0}, 'GLM-4.6', 'completed', 5, 6, 0, 0, 0, 11),
               -- 会话 B：窗口内 2 秒前有请求
               ('sess_bbb222', 'turn_b', {tb}, 'GLM-5.3', 'completed', 10, 20, 0, 0, 5, 30),
                -- 子代理会话：窗口内最活跃但不进列表
                ('sess_sub1', 'turn_s', {ts}, 'GLM-5.3', 'completed', 999, 999, 0, 0, 0, 1998),
                -- 两层子代理：必须与直接子代理同样并入主会话
                ('sess_sub1_child', 'turn_s2', {ts2}, 'GLM-5.3', 'completed', 20, 30, 0, 0, 5, 55);
             INSERT INTO message VALUES
               -- 会话 C：无 model_usage，仅用户消息落库（message 信号覆盖）
               ('msg_c1', 'sess_ccc333', {tc}, '{{\"role\":\"user\"}}');",
            ta = in_window(1_000),
            ta0 = in_window(3_600_000),
             tb = in_window(2_000),
             ts = in_window(500),
             ts2 = in_window(400),
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
        // ↑ = 50+5+999+15，两层子代理均并入
        assert_eq!(a.in_tokens, 1069);
        // ↓ = (200+6) + 999 + 30
        assert_eq!(a.out_tokens, 1235);
        assert_eq!(a.cache_read, 55);
        // Σ = ↑ + ↓ + ⟲（注入版 V15 口径）
        assert_eq!(a.total, 1069 + 1235 + 55);
        // × = 树内 model_usage 行数（每行一笔请求）
        assert_eq!(a.req_count, 4);
        // 子代理徽标：窗口内有活动的两层子代理数
        assert_eq!(a.sub_count, 2);
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
        // 今日合计（全库不分会话树；测试库全部行都在"今日"本地零点后）：
        // in = 100+5+999+10+20、out = 200+6+999+20+30、
        // ⟲ = 50+0+0+5+5、total = Σcomputed_total_tokens，× = 5 行
        let today = &snap.today_total;
        assert_eq!(today.in_tokens, 1134);
        assert_eq!(today.out_tokens, 1255);
        assert_eq!(today.cache_read, 60);
        assert_eq!(today.total, 2394, "total 应为 Σcomputed_total_tokens（主面板口径）");
        assert_eq!(today.req_count, 5);
        // 模型速度区：本测试库无 first_token_at/completed_at 列（老版本
        // 库形态），应降级为空数组而不是报错
        assert!(
            snap.model_speeds.is_empty(),
            "缺 first/completed 列的老库模型速度应降级为空"
        );

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
    fn 今日合计_本地零点口径与全库聚合降级() {
        use chrono::Timelike as _;
        let (conn, path) = hud_db("today");
        let now = 15_000_000_000_i64;
        let start = today_start_ms(now);
        // 零点落在 [now−24h, now] 内且为本地零点整点
        assert!(start <= now && start > now - 86_400_000, "{start}");
        let dt = chrono::DateTime::from_timestamp_millis(start).unwrap();
        let local = dt.with_timezone(&chrono::Local);
        assert_eq!((local.time().hour(), local.time().minute(), local.time().second()), (0, 0, 0));
        // 全库聚合（不分会话树）：零点后两行计入、零点前行剔除
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('s1', NULL, '/a'), ('s2', NULL, '/b');
             INSERT INTO model_usage VALUES
               ('s1', 't1', {t1}, 'M', 'completed', 100, 200, 0, 0, 40, 300),
               ('s2', 't2', {t2}, 'M', 'error', 10, 20, 0, 0, 5, 30),
               ('s1', 't0', {t0}, 'M', 'completed', 999, 999, 0, 0, 999, 1998);",
            t1 = now - 1_000,
            t2 = now - 2_000,
            t0 = start - 1, /* 零点前一行（昨日）不计入 */
        ))
        .unwrap();
        let today = collect_today_total(&conn, start).unwrap();
        assert_eq!(today.in_tokens, 110, "失败轮（error）同样计入今日合计");
        assert_eq!(today.out_tokens, 220);
        assert_eq!(today.cache_read, 45, "⟲ = 40 + 5（两行 cache_read 合计）");
        assert_eq!(today.total, 330, "total = Σcomputed_total_tokens");
        assert_eq!(today.req_count, 2);
        // 无今日行 → 全 0（前端不显示今日行）
        assert_eq!(
            collect_today_total(&conn, now - 500).unwrap(),
            HudTodayTotal::default()
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // computed_total_tokens 列缺失的老库：total 等价退化为 in+out
        let path2 = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-nocomputed.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path2);
        let conn = Connection::open(&path2).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER, model_id TEXT,
                status TEXT, input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER);",
        )
        .unwrap();
        conn.execute_batch("INSERT INTO model_usage VALUES
            ('s1', 't1', 1000, 'M', 'completed', 300, 50, 100);")
            .unwrap();
        let today = collect_today_total(&conn, 0).unwrap();
        assert_eq!(today.in_tokens, 300);
        assert_eq!(today.total, 350, "缺 computed 列时 total = in + out");
        drop(conn);
        let _ = fs::remove_file(&path2);
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

    /// 模型速度区测试库（model_usage 带完整时刻列的最小 schema）
    fn speed_zone_db(name: &str) -> (Connection, std::path::PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-session-hud-mszone-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch(
            "CREATE TABLE model_usage (
                session_id TEXT, turn_id TEXT, started_at INTEGER,
                model_id TEXT, status TEXT,
                first_token_at INTEGER, completed_at INTEGER,
                input_tokens INTEGER, output_tokens INTEGER);",
        )
        .unwrap();
        (conn, path)
    }

    #[test]
    fn 模型速度区_最近值窗口均值排序与上限降级() {
        let (conn, path) = speed_zone_db("zone");
        let now = 20_000_000_000_i64;
        let win_start = now - 10 * 60_000;
        // M-main：5 笔可信样本（100/100/100 常规 + 200 快样本 + 50 慢样本）
        //   → tps=100（最近一笔）、avg=700tok÷6.5s、max=200、min=50、samples=5
        // M-fast：1 笔可信样本（60 t/s），比 M-main 新（单样本时 max=min=tps）
        // M-dirty：error 行 / output=0 行 / 时序倒置行 → 无可信样本，不入列
        // M-old：窗口外 → 不参与
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               -- M-main 最早样本：1000ms 生成 100tok
               ('s1', 't1', {a1}, 'M-main', 'completed', {a1} + 500, {a1} + 1500, 10, 100),
               -- M-main 中间样本：500ms 生成 50tok
               ('s1', 't2', {a2}, 'M-main', 'completed', {a2} + 200, {a2} + 700, 10, 50),
               -- M-main 最新样本：3000ms 生成 300tok
               ('s1', 't3', {a3}, 'M-main', 'completed', {a3} + 1000, {a3} + 4000, 10, 300),
               -- M-main 更早的快样本：1000ms 生成 200tok → 200 t/s（最快）
               ('s1', 't10', {a4}, 'M-main', 'completed', {a4} + 100, {a4} + 1100, 10, 200),
               -- M-main 更早的慢样本：1000ms 生成 50tok → 50 t/s（最慢）
               ('s1', 't11', {a5}, 'M-main', 'completed', {a5} + 100, {a5} + 1100, 10, 50),
               -- M-fast：500ms 生成 30tok → 60 t/s
               ('s1', 't4', {b1}, 'M-fast', 'completed', {b1} + 100, {b1} + 600, 10, 30),
               -- M-dirty：error 状态（不计）
               ('s1', 't5', {c1}, 'M-dirty', 'error', {c1} + 100, {c1} + 600, 10, 100),
               -- M-dirty：output=0（无速度意义）
               ('s1', 't6', {c2}, 'M-dirty', 'completed', {c2} + 100, {c2} + 600, 10, 0),
               -- M-dirty：时序倒置 completed < first（除零防御跳过）
               ('s1', 't7', {c3}, 'M-dirty', 'completed', {c3} + 500, {c3} + 300, 10, 100),
               -- M-dirty：时刻 NULL（跳过）
               ('s1', 't8', {c4}, 'M-dirty', 'completed', NULL, NULL, 10, 100),
               -- M-old：窗口外（不参与）
               ('s1', 't9', {old}, 'M-old', 'completed', {old} + 100, {old} + 600, 10, 100);",
            a1 = now - 30_000,
            a2 = now - 20_000,
            a3 = now - 10_000,
            a4 = now - 40_000,
            a5 = now - 50_000,
            b1 = now - 5_000,
            c1 = now - 8_000,
            c2 = now - 7_000,
            c3 = now - 6_000,
            c4 = now - 6_500,
            old = now - 20 * 60_000,
        ))
        .unwrap();
        let speeds = collect_model_speeds(&conn, win_start);
        assert_eq!(speeds.len(), 2, "{speeds:?}");
        // 排序按最近使用降序：M-fast(−5s) > M-main(−10s)
        assert_eq!(speeds[0].model, "M-fast");
        assert!((speeds[0].tps - 60.0).abs() < 1e-9);
        assert!((speeds[0].avg_tps - 60.0).abs() < 1e-9);
        assert!(
            (speeds[0].max_tps - 60.0).abs() < 1e-9 && (speeds[0].min_tps - 60.0).abs() < 1e-9,
            "单样本时最快/最慢与最近值同值：{speeds:?}"
        );
        assert_eq!(speeds[0].samples, 1);
        assert_eq!(speeds[1].model, "M-main");
        // tps = 最近一笔（300tok ÷ 3000ms = 100 t/s）；avg = 700tok ÷ 6.5s；
        // 最快 = 200 t/s 快样本、最慢 = 50 t/s 慢样本（极端值不影响最近值）
        assert!((speeds[1].tps - 100.0).abs() < 1e-9);
        assert!((speeds[1].avg_tps - 700.0 * 1000.0 / 6_500.0).abs() < 1e-9);
        assert!(
            (speeds[1].max_tps - 200.0).abs() < 1e-9,
            "最快应为样本池内单笔最快（含非最近样本）：{speeds:?}"
        );
        assert!(
            (speeds[1].min_tps - 50.0).abs() < 1e-9,
            "最慢应为样本池内单笔最慢（含非最近样本）：{speeds:?}"
        );
        assert_eq!(speeds[1].samples, 5);
        // turn_id NULL 的后台请求行不过滤（速度语义与轮次无关）：
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ('s1', NULL, {n1}, 'M-null-turn', 'completed', {n1} + 100, {n1} + 1100, 10, 200);",
            n1 = now - 4_000,
        ))
        .unwrap();
        let speeds = collect_model_speeds(&conn, win_start);
        assert_eq!(speeds.len(), 3);
        assert_eq!(speeds[0].model, "M-null-turn", "NULL turn_id 后台请求计入");
        assert!((speeds[0].tps - 200.0).abs() < 1e-9);
        assert!(
            (speeds[0].max_tps - 200.0).abs() < 1e-9
                && (speeds[0].min_tps - 200.0).abs() < 1e-9,
            "单样本的最快/最慢与最近值同值：{speeds:?}"
        );

        // 上限截断：再补 2 个比 M-main 更新的模型 → 5 个可信模型只留
        // 最近 3 个（M-null-turn −4s、M-cap2 −2s、M-cap1 −1s）
        conn.execute_batch(&format!(
            "INSERT INTO model_usage VALUES
               ('s1', 'ta', {d1}, 'M-cap1', 'completed', {d1} + 100, {d1} + 600, 10, 50),
               ('s1', 'tb', {d2}, 'M-cap2', 'completed', {d2} + 100, {d2} + 600, 10, 50);",
            d1 = now - 1_000,
            d2 = now - 2_000,
        ))
        .unwrap();
        let speeds = collect_model_speeds(&conn, win_start);
        assert_eq!(speeds.len(), 3, "超过 3 个模型只留最近 3 个：{speeds:?}");
        assert_eq!(
            speeds
                .iter()
                .map(|s| s.model.as_str())
                .collect::<Vec<_>>(),
            vec!["M-cap1", "M-cap2", "M-null-turn"]
        );

        drop(conn);
        let _ = fs::remove_file(&path);

        // 老版本库缺 first/completed 列：降级为空数组不报错
        let (conn, path) = speed_zone_db("legacy");
        conn.execute_batch(&format!(
            "INSERT INTO model_usage (session_id, turn_id, started_at, model_id, status,
                input_tokens, output_tokens) VALUES
               ('s1', 't1', {t}, 'M', 'completed', 10, 100);",
            t = now - 1_000,
        ))
        .unwrap();
        assert!(collect_model_speeds(&conn, win_start).is_empty());
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度_请求级可信与近似降级() {
        let trusted = request_speed_at(
            &RequestTiming {
                output_tokens: 100,
                started_at: Some(1_000),
                first_token_at: Some(2_000),
                completed_at: Some(6_000),
                ..RequestTiming::default()
            },
            7_000,
        )
        .unwrap();
        assert_eq!(trusted.quality, SpeedQuality::Generation);
        assert!((trusted.value - 25.0).abs() < 1e-9, "{trusted:?}");

        // 短请求不套 1 秒下限：100 tokens / 10ms = 10,000 t/s。
        let short = request_speed_at(
            &RequestTiming {
                output_tokens: 100,
                first_token_at: Some(10_000),
                completed_at: Some(10_010),
                ..RequestTiming::default()
            },
            11_000,
        )
        .unwrap();
        assert!((short.value - 10_000.0).abs() < 1e-9, "{short:?}");

        let approximate = request_speed_at(
            &RequestTiming {
                output_tokens: 100,
                started_at: Some(20_000),
                duration_ms: Some(500),
                ..RequestTiming::default()
            },
            21_000,
        )
        .unwrap();
        assert_eq!(approximate.quality, SpeedQuality::RequestAverage);
        assert!((approximate.value - 200.0).abs() < 1e-9, "{approximate:?}");

        let reversed = RequestTiming {
            output_tokens: 100,
            first_token_at: Some(30_000),
            completed_at: Some(29_999),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&reversed, 31_000).is_none());
        let future = RequestTiming {
            output_tokens: 100,
            started_at: Some(40_000),
            duration_ms: Some(100),
            ..RequestTiming::default()
        };
        assert!(request_speed_at(&future, 40_050).is_none());
        assert!(request_speed_at(
            &RequestTiming {
                output_tokens: 0,
                started_at: Some(1),
                duration_ms: Some(1),
                ..RequestTiming::default()
            },
            2
        )
        .is_none());
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
                -- 主会话最新完成请求（应作为速度数据源；仅有 duration 时为请求平均速度）
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
        assert!((speed - 37.5).abs() < 1e-9, "{speed}");
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
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
        assert!((idle_speed - 37.5).abs() < 1e-9, "{idle_speed}");
        assert_eq!(m.speed_quality, Some(SpeedQuality::RequestAverage));
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

    #[test]
    fn 速度_最新进行中请求不泄漏同轮旧值() {
        let now = 9_600_000_000_i64;
        let path = std::env::temp_dir().join(format!(
            "zbar-session-hud-db-{}-speed-running.sqlite",
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
                time_to_first_token_ms INTEGER, first_token_at INTEGER,
                completed_at INTEGER);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('sess_running', NULL, '/running');
             INSERT INTO model_usage VALUES
                ('sess_running', 'turn_old', 9_599_997_000, 'M', 'completed',
                 10, 100, 0, 1_000, 100, 9_599_997_100, 9_599_998_000),
                ('sess_running', 'turn_new', 9_599_999_000, 'M', 'running',
                 10, 20, 0, NULL, NULL, NULL, NULL);",
        )
        .unwrap();

        let snapshot = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let brief = &snapshot.sessions[0];
        assert!(brief.generating);
        assert_eq!(brief.speed, None, "进行中最新请求不应泄漏上一笔速度");
        assert_eq!(brief.speed_state, SpeedState::Measuring);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 速度_新消息边界排除跨界完成的旧请求() {
        let (conn, path) = hud_db("speed-round-boundary");
        let now = 9_700_000_000_i64;
        // hud_db 的基础表没有速度列，补一个同名完整表无法 ALTER；单独
        // 使用一张带速度列的测试库，覆盖“旧请求在新消息之后才完成”的
        // 并发边界。
        drop(conn);
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
                input_tokens INTEGER, output_tokens INTEGER,
                cache_read_input_tokens INTEGER, duration_ms INTEGER,
                time_to_first_token_ms INTEGER, first_token_at INTEGER,
                completed_at INTEGER);
             CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER, data TEXT);
             INSERT INTO session VALUES ('sess_round', NULL, '/round');
             INSERT INTO turn_usage VALUES ('sess_round', 'turn_old', 'completed', 0);
             INSERT INTO model_usage VALUES
                ('sess_round', 'turn_old', 9699995000, 'M', 'completed',
                 10, 100, 0, 4500, 1000, 9699996000, 9699999500);
             INSERT INTO message VALUES
                ('msg_new', 'sess_round', 9699999000, '{\"role\":\"user\"}');",
        )
        .unwrap();

        let first = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let brief = &first.sessions[0];
        assert!(brief.generating, "新 user 消息应进入生成中状态");
        assert_eq!(brief.round_key.as_deref(), Some("message:msg_new"));
        assert_eq!(
            brief.speed, None,
            "started_at 早于新消息、但完成在消息之后的旧请求不得泄漏速度"
        );

        // 新轮请求开始后才允许产生当前轮速度；仍未落 turn_usage 时
        // generating 保持 true，速度仍来自已完成的 model_usage 请求。
        conn.execute(
            "INSERT INTO model_usage VALUES
             ('sess_round', 'turn_new', 9699999100, 'M', 'completed',
              10, 200, 0, 500, 100, 9699999200, 9699999600)",
            [],
        )
        .unwrap();
        let second = collect_session_snapshot(&conn, 10, now).unwrap().0;
        let brief = &second.sessions[0];
        assert!(brief.generating);
        assert_eq!(brief.round_key.as_deref(), Some("message:msg_new"));
        assert_eq!(brief.speed_quality, Some(SpeedQuality::Generation));
        assert!((brief.speed.unwrap() - 500.0).abs() < 1e-9);

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    /// rollout 单行测试 JSON（camelCase 键名与实测格式一致，结尾换行）
    fn line_json_with_id(
        start_ms: i64,
        output: i64,
        request_id: Option<&str>,
        duration_ms: i64,
    ) -> String {
        let ts = chrono::DateTime::from_timestamp_millis(start_ms)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let id = request_id
            .map(|value| format!(",\"requestId\":\"{value}\""))
            .unwrap_or_default();
        format!(
            "{{\"sessionId\":\"sess_x\",\"startedAt\":\"{ts}\"{id},\"durationMs\":{duration_ms},\"response\":{{\"usage\":{{\"outputTokens\":{output}}}}}}}\n"
        )
    }

    fn line_json(start_ms: i64, output: i64) -> String {
        line_json_with_id(start_ms, output, None, 500)
    }

    #[test]
    fn rollout解析_无效短请求与多种行形态() {
        let now = 10_000_000_000_i64;
        let ts = chrono::DateTime::from_timestamp_millis(now - 10)
            .unwrap()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true);
        let parsed = parse_rollout_line(
            format!(
                "{{\"sessionId\":\"sess_x\",\"startedAt\":\"{ts}\",\"durationMs\":10,\"response\":{{\"usage\":{{\"outputTokens\":100}}}}}}\n"
            )
            .as_bytes(),
        )
        .unwrap();
        let speed = request_speed_at(&parsed.timing, now).unwrap();
        assert_eq!(speed.quality, SpeedQuality::RequestAverage);
        assert!((speed.value - 10_000.0).abs() < 1e-9, "{speed:?}");
        assert!(parse_rollout_line(b"{not-json").is_none());
        assert!(parse_rollout_line(b"{\"startedAt\":\"bad\"}").is_none());
        assert!(parse_rollout_line(
            b"{\"startedAt\":\"1970-01-01T00:00:01Z\",\"response\":{}}"
        )
        .is_none());
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

        // 写入一行完整 JSON → 解析出 1 个请求，offset 推进到行尾
        std::fs::write(
            &path,
            line_json_with_id(now - 1_000, 100, Some("req-1"), 1_000),
        )
        .unwrap();
        tree.ingest(now);
        assert_eq!(tree.latest_rollout.as_ref().unwrap().value, 100.0);
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
        assert_eq!(tree.latest_rollout.as_ref().unwrap().value, 100.0, "半行不应解析");
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
        assert!(tree.latest_rollout.is_some());
        assert!(
            (tree.latest_rollout.as_ref().unwrap().value - 200.0).abs() < 1e-9,
            "半行补全后应解析新请求"
        );
        assert_eq!(tree.cursors[0].offset, path.metadata().unwrap().len());

        // 同一 requestId 重复出现时只保留一次，不改变最新值。
        let duplicate = line_json_with_id(now - 500, 999, Some("req-1"), 500);
        std::fs::write(&path, duplicate).unwrap();
        tree.cursors[0].offset = 0;
        tree.ingest(now);
        assert!((tree.latest_rollout.as_ref().unwrap().value - 200.0).abs() < 1e-9);

        // truncate/重建：文件变小 → offset 归零重读，不报错
        let previous_offset = tree.cursors[0].offset;
        std::fs::write(
            &path,
            line_json_with_id(now - 200, 700, Some("r2"), 200),
        )
        .unwrap();
        assert!(path.metadata().unwrap().len() < previous_offset);
        tree.ingest(now);
        assert_eq!(tree.cursors[0].offset, path.metadata().unwrap().len());
        // 短跨度按真实 200ms 计算：700 / 0.2s = 3500 t/s，无一秒下限。
        assert!((tree.latest_rollout.as_ref().unwrap().value - 3_500.0).abs() < 1e-9);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 速度_展示策略_无请求ID时按完成时刻选择rollout参考() {
        let now = 12_000_000_000_i64;
        let mut tree = TreeSpeed::new("t", &["t".into()], None);
        let db = SpeedSnapshot {
            value: 7.0,
            quality: SpeedQuality::Generation,
            completed_at: now - 2_000,
            request_id: None,
        };
        assert_eq!(tree.display_speed(true, Some(db.clone()), now), Some(db.clone()));

        tree.latest_rollout = Some(SpeedSnapshot {
            value: 11.0,
            quality: SpeedQuality::RequestAverage,
            completed_at: now - 1_000,
            request_id: Some("rollout-1".into()),
        });
        let picked = tree.display_speed(true, Some(db.clone()), now).unwrap();
        assert_eq!(picked.value, 11.0);
        assert_eq!(picked.quality, SpeedQuality::RequestAverage);

        // A newer but invalid rollout row clears the older DB fallback;
        // equal timestamps still allow the DB value to win.
        tree.latest_rollout = None;
        tree.latest_rollout_order = Some(now - 500);
        assert!(tree.display_speed(false, Some(db.clone()), now).is_none());
        tree.latest_rollout_order = Some(db.completed_at);
        assert_eq!(tree.display_speed(false, Some(db.clone()), now), Some(db.clone()));

        // 新轮尚无完成请求：旧 DB/rollout 速度都被排除，返回 None。
        tree.observe_round(Some("message:round-2"), Some(now), true);
        assert!(tree.display_speed(true, Some(db), now + 100).is_none());

        // 新轮完成请求出现后可显示，不随秒级 tick 衰减，也不保持旧值。
        tree.latest_rollout = Some(SpeedSnapshot {
            value: 13.0,
            quality: SpeedQuality::Generation,
            completed_at: now + 200,
            request_id: Some("rollout-2".into()),
        });
        let current = tree.display_speed(true, None, now + 300).unwrap();
        assert_eq!(current.value, 13.0);
    }

    #[test]
    fn 速度_快速连续发问取消与rollout缺失不复用上一轮() {
        let now = 12_500_000_000_i64;
        let mut tree = TreeSpeed::new("t", &["t".into()], None);
        let old = SpeedSnapshot {
            value: 80.0,
            quality: SpeedQuality::Generation,
            completed_at: now - 2_000,
            request_id: Some("old".into()),
        };
        tree.latest_rollout = Some(old.clone());
        tree.latest_rollout_order = Some(old.completed_at);
        tree.observe_round(Some("message:round-1"), Some(now - 4_000), true);
        assert_eq!(tree.display_speed(true, Some(old.clone()), now), Some(old.clone()));

        // 两轮之间没有一次 generating=false 的快照，消息 id 仍能触发清理。
        tree.observe_round(Some("message:round-2"), Some(now - 500), true);
        assert!(tree.display_speed(true, Some(old.clone()), now).is_none());

        // 新轮被取消后仍带 round-2 身份；idle 分支不可把 round-1 rollout
        // 或 DB fallback 重新带回来。
        tree.observe_round(Some("message:round-2"), Some(now - 500), false);
        assert!(tree.display_speed(false, Some(old), now).is_none());

        // rollout 文件缺失也不改变当前轮资格：没有新请求就保持无值。
        tree.latest_rollout = None;
        assert!(tree.display_speed(false, None, now + 1_000).is_none());

        // 旧请求即使在新消息之后才追加到 rollout，也按 startedAt 排除；
        // 不能仅用 completedAt 与新轮边界比较。
        let dir = test_dir("round-boundary-rollout");
        let path = dir.join("model-io-t.jsonl");
        tree.cursors = vec![RolloutCursor {
            path: path.clone(),
            offset: 0,
        }];
        fs::write(
            &path,
            line_json_with_id(now - 800, 100, Some("old-after-message"), 700),
        )
        .unwrap();
        tree.ingest(now);
        assert!(tree.latest_rollout.is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 速度_无用户消息时以turn_id识别快速连续轮() {
        let now = 12_600_000_000_i64;
        let mut tree = TreeSpeed::new("t", &["t".into()], None);
        let old = SpeedSnapshot {
            value: 20.0,
            quality: SpeedQuality::RequestAverage,
            completed_at: now - 1_000,
            request_id: None,
        };
        tree.observe_round(Some("turn:turn-1"), Some(now - 2_000), false);
        tree.latest_rollout = Some(old.clone());
        tree.latest_rollout_order = Some(old.completed_at);
        tree.observe_round(Some("turn:turn-2"), Some(now - 100), true);
        assert!(tree.display_speed(true, Some(old), now).is_none());
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

    #[test]
    fn 生成中_超期孤儿消息不再永久驱动生成态() {
        // A1 悬浮版：12 小时前的未匹配 user 消息晚于树内最新请求，但超出
        // PENDING_FRESH_MS 且无进行中轮佐证 → 不再视为生成中（窗口开大
        // 让会话仍因该消息出现在列表里，验证状态本身回落空闲）
        let (conn, path) = hud_db("msg-stale");
        let now = 14_000_000_000_i64;
        let stale = now - 12 * 3600 * 1000;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_m', NULL, '/m');
             INSERT INTO model_usage VALUES
               ('sess_m', 't_old', {t_old}, 'M', 'completed', 10, 10, 0, 0, 0, 20);
             ALTER TABLE model_usage ADD COLUMN first_token_at INTEGER;
             ALTER TABLE model_usage ADD COLUMN completed_at INTEGER;
             UPDATE model_usage SET first_token_at = {t_first}, completed_at = {t_end}
               WHERE turn_id = 't_old';
             INSERT INTO turn_usage VALUES
               ('sess_m', 't_old', 'completed', {t_old});
             INSERT INTO message VALUES
               ('msg_stale', 'sess_m', {stale}, '{{\"role\":\"user\"}}');",
            t_old = stale - 60_000,
            t_first = stale - 59_000,
            t_end = stale - 58_000,
        ))
        .unwrap();
        // 窗口 24 小时：陈旧消息让会话保留可见，但生成态必须回落
        let snap = collect_session_snapshot(&conn, 24 * 60, now).unwrap().0;
        assert_eq!(snap.sessions.len(), 1, "{snap:?}");
        let m = &snap.sessions[0];
        assert!(!m.generating, "超期孤儿消息不得永久冒充活跃轮: {m:?}");
        assert_eq!(m.speed_state, SpeedState::Recent, "空闲会话显示最近确认速度");

        // 新鲜期内同形态消息 → 仍驱动生成中（首请求等待）
        conn.execute_batch(&format!(
            "DELETE FROM message WHERE id = 'msg_stale';
             INSERT INTO message VALUES
               ('msg_fresh', 'sess_m', {fresh}, '{{\"role\":\"user\"}}');",
            fresh = now - 2_000,
        ))
        .unwrap();
        let snap = collect_session_snapshot(&conn, 24 * 60, now).unwrap().0;
        assert!(snap.sessions[0].generating, "新鲜消息应驱动生成中");
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
        // rollout 只保留最新有效请求；不做窗口加权或秒级衰减。
        let display = tree.display_speed(true, None, now);
        eprintln!(
            "文件 {} 是否解析到有效请求 {}，展示值 {:?}",
            newest.display(),
            tree.ever_parsed,
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

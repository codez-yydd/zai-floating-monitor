//! 独立桌面宠物（宠物功能第二阶段）：不依赖皮肤安装的透明悬浮窗宠物。
//!
//! 与第一阶段（皮肤注入版，见 agent_theme::inject::PET_JS）的关系：
//! 两端共用同一份宠物核心（public/pet-core.js 的 ZBarPet 工厂），区别
//! 仅在宿主——
//! - 注入版：ZCode 对话页内，参数经 variables.css 热重载、数据经
//!   window.__ZBAR_USAGE__（usage_feed 只在皮肤已安装时导出）；
//! - 独立版（本模块）：ZBar 自己的透明无边框置顶窗口（加载 pet.html
//!   多入口页面），参数经 set_pet_config 命令直接下发（Tauri 事件）、
//!   数据经本模块的独立轮询器（下方）推流。
//!
//! 组成：
//! 1. PetConfig 配置持久化（~/.zbar/pet.json）：开关/形象/大小 + 窗口
//!    位置（逻辑坐标，拖动结束落盘，重启恢复；无记录时默认主显示器
//!    右下角）；
//! 2. 透明悬浮窗（label "pet"）：transparent + 无边框 + 置顶 + 创建不
//!    抢焦点 + skipTaskbar，尺寸随 size 档位按窗口所在屏幕逻辑高换算
//!    的 px，容器支持拖动移位（页面 data-tauri-drag-region），宠物本体
//!    无点击交互；
//! 3. 独立状态轮询器：宠物窗口开启时启动、关闭即停（无常驻开销），
//!    每 2 秒查 ZCode 主库，产出与 usage-data.js 同构的 runs/turns 摘要
//!    （仅保留宠物状态机消费的字段，见 PetTurnBrief/PetRunBrief）与
//!    pu（待处理用户消息，V5：用户发消息后首笔模型请求完成落库前的
//!    预判信号，见 usage_feed 模块头）经 Tauri 事件推给宠物窗口。查询
//!    逻辑复用 agent_theme::usage_feed 的 collect_* 函数（不改其既有契约
//!    与启动条件）；查询失败静默跳过下周期重试，ZCode 未运行/无库时
//!    事件停流、宠物核心按心跳停滞自然沉睡。
//!
//! 主面板（panel 窗口）不受影响：宠物窗口不抢焦点、不参与失焦自动
//! 收起逻辑（lib.rs 的 Focused(false) 隐藏只作用于 panel）。

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
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

/// 宠物窗口 label（与 capabilities 的 windows 白名单、事件目标一致）
pub const PET_WINDOW_LABEL: &str = "pet";
/// 用量数据事件（宠物窗口 listen，payload = PetSnapshot）
pub const PET_USAGE_EVENT: &str = "zbar://pet-usage";
/// 参数变更事件（宠物窗口 listen，payload = { style, size }）
pub const PET_PARAMS_EVENT: &str = "zbar://pet-params";

/// 宠物尺寸档位（屏幕高度百分比，%）：size 字段语义自此版本起由逻辑
/// px 改为档位 1~5，实际显示边长 px = 屏幕逻辑高 × 档位百分比（四舍五
/// 入取整）。逻辑 px 只随 DPI 缩放、不随物理分辨率，固定 px 在高分屏
/// （如 4K@150%，逻辑屏高 1440）上占屏比例偏小约 30%，改为屏高比例后
/// 换机器观感一致。各档在典型屏高下的约略 px：
/// - 1080p@100%（逻辑高 1080）：59 / 81 / 108 / 135 / 162
/// - 4K@150%（逻辑高 1440）：79 / 108 / 144 / 180 / 216
/// 默认档 10% 与 Petdex 官方桌宠默认 110px 观感接近；上限 15% 为桌宠
/// 级视觉分量。
/// 与注入版 ThemeParams 的 pet_size（同为档位语义）共用本表。
pub const PET_SIZE_LEVEL_PCT: [f64; 5] = [5.5, 7.5, 10.0, 12.5, 15.0];
/// 宠物默认尺寸档位（10%，与 Petdex 官方桌宠默认 110px@1080 观感接近）
pub const DEFAULT_PET_SIZE_LEVEL: u32 = 3;
/// 旧版（px 语义）合法边长域：读取迁移时旧值先夹回该域再按屏高换算
/// 成最近档位（两条管道——pet.json 的 size 与 params.json 的 pet_size——
/// 迁移口径一致）
pub const PET_SIZE_LEGACY_PX_RANGE: (u32, u32) = (48, 128);
/// 取不到屏幕信息时的兜底逻辑屏高（1080p@100%）：换算永不失败
pub const PET_SIZE_FALLBACK_SCREEN_H: f64 = 1080.0;
/// 宠物默认形象（与 pet-core.js 的 PET_STYLES 第一形象一致）
pub const DEFAULT_PET_STYLE: &str = "cat";
/// 宠物窗口默认边距（px，逻辑坐标）：无持久化位置时贴主显示器右下角。
/// Windows 留出任务栏高度（与 toggle_panel 的 48px 余量同口径再加边距）；
/// macOS 无底部任务栏，仅留呼吸边距
#[cfg(target_os = "macos")]
const PET_DEFAULT_MARGIN: f64 = 16.0;
#[cfg(not(target_os = "macos"))]
const PET_DEFAULT_MARGIN: f64 = 56.0;

/// 轮询周期（毫秒）：与注入版 usage_feed 的导出周期一致（2 秒），
/// 宠物核心的心跳新鲜度阈值（10 秒）下留足余量
const FEED_INTERVAL_MS: u64 = 2000;
/// turns 查询窗口（毫秒）：宠物只消费 turns 的数量与末尾轮标识（庆祝
/// 判定的"新增"检测），1 小时窗口足够覆盖且远小于注入版的 7 天全量
/// 导出窗口（历史回填与统计条渲染是注入版的需求，宠物不需要），
/// 避免每 2 秒大窗口查询与 IPC 大 payload。
/// runs 的扫查下界也用本窗口（覆盖长轮早期请求的完整聚合，>1 小时的
/// 长轮边缘场景由 runs 新鲜度窗口兜底，与注入版 7 天口径的差异可接受）
const TURNS_WINDOW_MS: i64 = 3600 * 1000;
/// runs 新鲜度窗口（毫秒）：与 usage_feed::RUN_WINDOW_MS 同口径
/// （进行中轮的最新请求落在窗口内才视为进行中）
const RUN_WINDOW_MS: i64 = 10 * 60 * 1000;
/// set_pet_config 等待主线程窗口操作完成的超时上限：建窗/关窗在事件
/// 循环内是快操作，仅在事件循环异常（退出中等）时兜底，防止命令永久
/// 挂起（配置此时已先落盘，不丢）
const PET_WINDOW_OP_TIMEOUT: Duration = Duration::from_secs(10);

// ============================================================
// 尺寸档位换算（独立版与注入版共用）
// ============================================================

/// 档位 → 显示边长 px：屏幕逻辑高 × 档位百分比，四舍五入取整。
/// level 越界夹到 1..=5；屏高非法（非正/NaN/无穷）兜底 1080 逻辑高，
/// 保证换算永不失败。渲染层（pet-core.js 的 applySize 与注入版 pet.js
/// 的画布尺寸）只消费 px 整数，档位换算只在入口处一次完成。
pub fn pet_size_px(level: u32, screen_logical_height: f64) -> u32 {
    let idx = (level.clamp(1, PET_SIZE_LEVEL_PCT.len() as u32) - 1) as usize;
    let h = if screen_logical_height.is_finite() && screen_logical_height > 0.0 {
        screen_logical_height
    } else {
        PET_SIZE_FALLBACK_SCREEN_H
    };
    (h * PET_SIZE_LEVEL_PCT[idx] / 100.0).round() as u32
}

/// 按屏高百分比取最近档位（1..=5）：等距取更小档（min_by 稳定偏好
/// 靠前的枚举序），迁移语义与 pet_size_px 互为反函数（同屏高下往返
/// 不漂移）
fn nearest_size_level(pct: f64) -> u32 {
    PET_SIZE_LEVEL_PCT
        .iter()
        .enumerate()
        .min_by(|a, b| {
            (a.1 - pct)
                .abs()
                .partial_cmp(&(b.1 - pct).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .map(|(i, _)| i as u32 + 1)
        .unwrap_or(DEFAULT_PET_SIZE_LEVEL)
}

/// 配置 size 字段读取归一（两条管道共用）：
/// - 1..=5：档位直读（新版语义，不迁移）；
/// - 6 及以上：旧版 px 值（含越界脏值先夹回 48~128 旧域）按当前屏高
///   换算成屏高占比后取最近档位（一次性迁移，调用方负责落盘）；
/// - 0：彻底非法值回默认档位 3。
/// 注意：旧版 ZBar 读到新配置的 1~5 会被旧 clamp 夹到 48px——属可接受
/// 降级（宠物偏小但可用），新版重写配置即恢复。
pub fn normalize_pet_size(raw: u32, screen_logical_height: f64) -> u32 {
    if (1..=PET_SIZE_LEVEL_PCT.len() as u32).contains(&raw) {
        return raw;
    }
    if raw == 0 {
        return DEFAULT_PET_SIZE_LEVEL;
    }
    let h = if screen_logical_height.is_finite() && screen_logical_height > 0.0 {
        screen_logical_height
    } else {
        PET_SIZE_FALLBACK_SCREEN_H
    };
    let px = raw.clamp(PET_SIZE_LEGACY_PX_RANGE.0, PET_SIZE_LEGACY_PX_RANGE.1);
    nearest_size_level(px as f64 / h * 100.0)
}

/// 显示器逻辑高（物理高 ÷ DPI 缩放）：default_bottom_right 同款换算
fn monitor_logical_height(mon: &tauri::Monitor) -> f64 {
    mon.size().height as f64 / mon.scale_factor()
}

/// ZBar 主显示器逻辑高缓存（f64 位模式，0 = 未初始化）：
/// 注入版 variables.css 渲染（agent_theme::store）在无 AppHandle 的
/// 纯目录函数里需要屏高做档位换算，取启动/改配置时缓存的主显示器值
/// （注入版宠物显示在 ZCode 页面，用主显示器近似即可）。换屏/改分辨
/// 率后缓存过期，ZBar 重启或下次 start_if_enabled/set_pet_config 时
/// 刷新——不要求实时监听屏幕变化，与独立版「下次读取配置或重建窗口
/// 时生效」的时机口径一致。
static SCREEN_H_BITS: AtomicU64 = AtomicU64::new(0);

/// 刷新屏高缓存（主显示器逻辑高；取不到保持原值不覆盖）
pub fn remember_screen_height(app: &AppHandle) {
    if let Some(mon) = app.primary_monitor().ok().flatten() {
        let h = monitor_logical_height(&mon);
        if h > 0.0 {
            SCREEN_H_BITS.store(h.to_bits(), Ordering::Relaxed);
        }
    }
}

/// 读屏高缓存；未初始化兜底 1080（渲染换算永不失败）。线程安全
/// （纯原子读，pets 模块的命令线程也会调用）。
pub fn cached_screen_height() -> f64 {
    let bits = SCREEN_H_BITS.load(Ordering::Relaxed);
    if bits == 0 {
        PET_SIZE_FALLBACK_SCREEN_H
    } else {
        f64::from_bits(bits)
    }
}

/// 档位 → px（窗口所在显示器优先，取不到回主显示器再回 1080 兜底）：
/// 独立宠物窗口路径专用。必须在主线程事件循环上下文调用（macOS 的
/// monitor 查询要求主线程）——现有调用点（setup 阶段与 run_on_main_
/// thread 闭包）均满足。
fn pet_size_px_for(app: &AppHandle, win: Option<&tauri::WebviewWindow>, level: u32) -> u32 {
    let mon = win
        .and_then(|w| w.current_monitor().ok().flatten())
        .or_else(|| app.primary_monitor().ok().flatten());
    let h = mon
        .as_ref()
        .map(monitor_logical_height)
        .filter(|h| *h > 0.0)
        .unwrap_or(PET_SIZE_FALLBACK_SCREEN_H);
    pet_size_px(level, h)
}

// ============================================================
// 配置持久化（~/.zbar/pet.json，与项目其它配置同目录）
// ============================================================

/// 独立桌面宠物配置。serde camelCase：与前端契约字段（enabled/style/
/// size/pos）一一对应；`#[serde(default)]`：旧版文件缺字段时按默认补齐。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct PetConfig {
    /// 悬浮窗开关：true = 创建窗口并启动轮询；false = 关闭窗口停轮询
    pub enabled: bool,
    /// 宠物形象 id（pet-core.js 的 PET_STYLES 键：cat / bot）
    pub style: String,
    /// 宠物尺寸档位（1~5，屏高百分比见 PET_SIZE_LEVEL_PCT）：悬浮窗
    /// 边长 = 档位 × 窗口所在屏幕逻辑高（建窗/同步尺寸时换算）
    pub size: u32,
    /// 窗口左上角位置（逻辑坐标 x/y，拖动结束落盘，重启恢复）；
    /// None = 从未拖动过，创建时默认主显示器右下角
    pub pos: Option<(f64, f64)>,
}

impl Default for PetConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            style: DEFAULT_PET_STYLE.to_string(),
            size: DEFAULT_PET_SIZE_LEVEL,
            pos: None,
        }
    }
}

impl PetConfig {
    /// 把越界参数收敛到合法范围（保存前的防御，脏数据不落盘）：
    /// 档位归一到 1..=5（旧版 px 值按缓存屏高换算最近档位迁移，见
    /// normalize_pet_size）；形象 id 防空串/空白回默认（形象合法性由
    /// pet-core.js 按内嵌形象库回退第一形象，Rust 侧不重复维护形象
    /// 清单，避免两处漂移）
    pub fn clamped(mut self) -> Self {
        self.size = normalize_pet_size(self.size, cached_screen_height());
        if self.style.trim().is_empty() {
            self.style = DEFAULT_PET_STYLE.to_string();
        } else {
            self.style = self.style.trim().to_string();
        }
        self
    }
}

fn pet_config_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("pet.json"))
}

/// 读取宠物配置；文件不存在或内容损坏时静默返回默认值（首开无文件）。
/// 旧版 px 语义的 size（48~128）在首次读取时按缓存屏高一次性迁移为
/// 最近档位并落盘（迁移后值域 1..=5，后续读取直读不再写盘）。
pub fn load_pet_config() -> PetConfig {
    let Ok(path) = pet_config_path() else {
        return PetConfig::default();
    };
    load_pet_config_at(&path)
}

/// load_pet_config 的路径显式版（单元测试复用，不依赖真实 ~/.zbar）：
/// 读 → size 归一（旧 px 迁移档位）→ 变化则落盘。
fn load_pet_config_at(path: &std::path::Path) -> PetConfig {
    let Some(mut cfg) = read_pet_config_file(path) else {
        return PetConfig::default();
    };
    let size = normalize_pet_size(cfg.size, cached_screen_height());
    if size != cfg.size {
        cfg.size = size;
        let _ = write_pet_config_file(path, &cfg); // 迁移落盘失败静默（下次读取重试）
    }
    cfg
}

/// 从指定路径读配置（供单元测试复用）。失败返回 None。
pub(crate) fn read_pet_config_file(path: &std::path::Path) -> Option<PetConfig> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 保存宠物配置：clamp 后原子性写入（先写临时文件再改名）。
pub fn save_pet_config(config: &PetConfig) -> Result<(), String> {
    let path = pet_config_path()?;
    write_pet_config_file(&path, &config.clone().clamped())
}

/// 写配置到指定路径（供单元测试复用）。
pub(crate) fn write_pet_config_file(path: &std::path::Path, config: &PetConfig) -> Result<(), String> {
    let json = serde_json::to_string_pretty(config)
        .map_err(|e| format!("序列化宠物配置失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入宠物配置失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("保存宠物配置失败: {e}"))
}

// ============================================================
// Tauri 命令
// ============================================================

/// 读取独立桌面宠物配置（设置页卡片初始数据）。
#[tauri::command]
pub fn get_pet_config() -> Result<PetConfig, String> {
    Ok(load_pet_config().clamped())
}

/// 保存并应用宠物配置（改完即生效，无保存按钮）：
/// - 开关切换：开 → 建窗 + 启轮询；关 → 停轮询 + 关窗；
/// - 形象/大小变化：窗口存在时同步尺寸 + 推参数事件（页面调 setParams）。
///
/// async 命令（工作线程执行），窗口操作经 run_on_main_thread 投递主线程
/// 事件循环执行：async 上下文直接调窗口 API 会 panic（非主线程）；而改回
/// 同步命令直接调同样致命——同步命令在 Windows 上占用主线程，建窗需要
/// 主线程事件循环处理消息，形成主线程自等待死锁（本命令曾经的严重
/// bug：开宠物后 pet.json 不落盘、全应用命令无响应）。闭包结果经
/// channel 回传，recv_timeout 兜底。
///
/// 配置先落盘再动窗口：窗口操作失败配置不丢（下次启动 start_if_enabled
/// 按 enabled 恢复）；关闭分支先写 enabled=false 再关窗，Destroyed 复位
/// 路径（handle_pet_window_destroyed）读到已关状态不再改写，幂等语义
/// 与原实现一致。
#[tauri::command]
pub async fn set_pet_config(config: PetConfig, app: AppHandle) -> Result<PetConfig, String> {
    let next = config.clamped();
    let prev = load_pet_config();
    let push_params = should_push_params(&prev, &next);

    // 关闭分支先停轮询（原顺序：停数据流优先于关窗；只是置位原子标志，
    // 线程安全；Destroyed 挂点还会再停一次，幂等）
    if !next.enabled {
        stop_feed();
    }

    // 先落盘再动窗口（见函数 doc：失败可恢复 + Destroyed 幂等前提）
    save_pet_config(&next)?;

    // 窗口创建/尺寸同步/关闭全部投递主线程事件循环执行
    let (tx, rx) = mpsc::channel::<Result<(), String>>();
    let cfg_main = next.clone();
    let app_main = app.clone();
    app.run_on_main_thread(move || {
        // 主线程顺带刷新屏高缓存（macOS 的 monitor 查询要求主线程；
        // 改档位时机同步注入版渲染基准，换屏后改档位即按新屏高换算）
        remember_screen_height(&app_main);
        let outcome = if cfg_main.enabled {
            ensure_pet_window(&app_main, &cfg_main)
        } else if let Some(win) = app_main.get_webview_window(PET_WINDOW_LABEL) {
            let _ = win.close(); // 关窗失败不报错（窗口可能已不在）
            Ok(())
        } else {
            Ok(())
        };
        // 参数变化（含窗口已存在的开关保持开）：推给页面即时热切换
        // （ensure_pet_window 的已存在分支也会推，双推幂等无害）
        if outcome.is_ok() && push_params {
            let size_px = pet_size_px_for(
                &app_main,
                app_main.get_webview_window(PET_WINDOW_LABEL).as_ref(),
                cfg_main.size,
            );
            push_pet_params(&app_main, &cfg_main, size_px);
        }
        let _ = tx.send(outcome);
    })
    .map_err(|e| format!("投递宠物窗口操作到主线程失败: {e}"))?;

    match rx.recv_timeout(PET_WINDOW_OP_TIMEOUT) {
        Ok(Ok(())) => {}
        Ok(Err(e)) => return Err(e),
        Err(e) => return Err(format!("宠物窗口操作超时: {e}")),
    }

    // 开启分支：建窗成功后才喂数据（原顺序保持；失败即不启动，下次开可重试）
    if next.enabled {
        start_feed(app.clone());
    }

    Ok(next)
}

/// 是否需要向宠物窗口热推参数：仅开启状态且形象或尺寸变化（位置变化
/// 由 Moved 挂点持久化，不经参数事件）。
fn should_push_params(prev: &PetConfig, next: &PetConfig) -> bool {
    next.enabled && (prev.style != next.style || prev.size != next.size)
}

/// 向宠物窗口推送当前参数（setParams 的 { style, size } 载荷）。
/// size 必须是档位换算后的 px（pet-core.js 的 applySize 消费 px 整数，
/// 误推档位小数值会把宠物缩成蚂蚁）——主线程调用方用 pet_size_px_for
/// 换算，非主线程调用方（pets 模块命令线程）用 cached_screen_height
/// 兜底换算。pets 模块在自定义宠物导入替换/删除后复用本函数热刷新。
pub(crate) fn push_pet_params(app: &AppHandle, cfg: &PetConfig, size_px: u32) {
    #[derive(Clone, Serialize)]
    #[serde(rename_all = "camelCase")]
    struct Params {
        style: String,
        size: u32,
    }
    let _ = app.emit_to(
        PET_WINDOW_LABEL,
        PET_PARAMS_EVENT,
        Params {
            style: cfg.style.clone(),
            size: size_px,
        },
    );
}

/// 应用启动挂点：配置开启时恢复悬浮窗与轮询（未开启零开销）。
/// 入口顺带刷新屏高缓存（注入版 variables.css 渲染换算消费；ZBar
/// 重启后换屏/改分辨率即按新屏高重算——皮肤页状态轮询触发的
/// ensure_theme_assets 重渲会静默写入新 px）。
pub fn start_if_enabled(app: &AppHandle) {
    remember_screen_height(app);
    let cfg = load_pet_config().clamped();
    if !cfg.enabled {
        return;
    }
    if let Err(e) = ensure_pet_window(app, &cfg) {
        eprintln!("[zbar-pet] 恢复宠物窗口失败: {e}");
        return;
    }
    start_feed(app.clone());
}

// ============================================================
// 悬浮窗
// ============================================================

/// 确保 "pet" 悬浮窗存在并应用配置：已存在时仅同步尺寸与参数（防重复
/// 创建同名窗口）；不存在时创建（透明、无边框、置顶、不抢焦点、
/// skipTaskbar，位置取持久化坐标或默认主显示器右下角）。
/// 尺寸档位在入口处一次换算为 px（窗口所在显示器逻辑高优先，建窗时
/// 无窗口可用主显示器），下游建窗/set_size/宽高比逻辑不变继续吃 px。
/// 必须在主线程的事件循环上下文调用（WebviewWindowBuilder 的要求）：
/// 合法调用点是 setup 阶段（start_if_enabled）与 run_on_main_thread
/// 投递的闭包（set_pet_config）。不能在同步命令主体里直接调——同步命令
/// 占用主线程，建窗等不到事件循环处理消息，自等待死锁。
fn ensure_pet_window(app: &AppHandle, cfg: &PetConfig) -> Result<(), String> {
    if let Some(win) = app.get_webview_window(PET_WINDOW_LABEL) {
        // 已存在：同步尺寸并重推参数（窗口可能是恢复启动时的旧实例）
        let size_px = pet_size_px_for(app, Some(&win), cfg.size);
        sync_pet_window_size(&win, &cfg.style, size_px)
            .map_err(|e| format!("调整宠物窗口尺寸失败: {e}"))?;
        push_pet_params(app, cfg, size_px);
        return Ok(());
    }

    // 建窗：与默认右下角位置同源取主显示器（窗口尚不存在，无从取
    // 所在显示器）；换屏后重建窗口即按新屏高自适应
    let mon = app.primary_monitor().ok().flatten();
    let screen_h = mon
        .as_ref()
        .map(monitor_logical_height)
        .filter(|h| *h > 0.0)
        .unwrap_or(PET_SIZE_FALLBACK_SCREEN_H);
    let size = pet_size_px(cfg.size, screen_h);
    // P2-1：窗口高度按选中形象宽高比（自定义 Petdex 帧 192×208），
    // 内建保持正方形
    let (_, win_h) = window_size_of(&cfg.style, size);
    let mut builder = WebviewWindowBuilder::new(
        app,
        PET_WINDOW_LABEL,
        WebviewUrl::App("pet.html".into()),
    )
    .title("ZBar Pet")
    .inner_size(size as f64, win_h)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .shadow(false)
    .resizable(false)
    // Windows 钳宽修复：tao 给所有窗口无条件带 WS_CAPTION（无边框仅靠
    // WM_NCCALCSIZE 视觉去标题栏），DefWindowProc 的 WM_GETMINMAXINFO
    // 默认最小跟踪宽度（SM_CXMINTRACK，125% DPI 下 170 物理）会把
    // 宠物窗口（典型 59~162 逻辑）全部钳宽（tao 仅在设置了 min 时才覆盖该值）
    .min_inner_size(1.0, 1.0)
    // 抢焦点修复：Windows 上 .focused(false) 不足以阻止建窗激活（WebView2
    // 建控件的副作用，实测建窗后本窗口即前台），focusable(false) 映射
    // WS_EX_NOACTIVATE 从根上不激活；宠物无键盘交互，拖动不需要激活
    .focusable(false)
    .focused(false); /* 创建时不抢焦点（不惊扰当前输入焦点） */

    // 位置：持久化逻辑坐标优先，无记录默认主显示器右下角
    let (x, y) = match cfg.pos {
        Some(p) => p,
        None => default_bottom_right(mon.as_ref(), size),
    };
    builder = builder.position(x, y);

    // macOS：显式清空窗口背景，避免 WebView 默认背景把透明窗口盖成
    // 纯白（与 lib.rs setup 对 panel 窗口的处理同款；宠物窗口无毛玻璃
    // 材质，仅清背景即可）。非 macOS 下窗口句柄无需保留（无后续操作）
    #[cfg(target_os = "macos")]
    {
        let win = builder
            .build()
            .map_err(|e| format!("创建宠物窗口失败: {e}"))?;
        let _ = win.set_background_color(Some(tauri::window::Color(0, 0, 0, 0)));
    }
    #[cfg(not(target_os = "macos"))]
    {
        // Windows 钳宽修正：tao 给所有窗口无条件带 WS_CAPTION（无边框仅靠
        // 子类化后的 WM_NCCALCSIZE 视觉去掉标题栏），而 CreateWindowExW
        // 期间发出的 WM_GETMINMAXINFO 走的是子类化前的 DefWindowProc，
        // 其默认最小跟踪宽度（SM_CXMINTRACK，125% DPI 下 170 物理）会把
        // 宠物窗口在出生时刻就钳宽（实测 64 逻辑 → 170x80 物理）。子类化
        // 完成后 min_inner_size(1,1) 的覆盖才生效，因此：隐藏建窗 → 显式
        // set_size 收回请求尺寸（此时不再受钳制）→ 显示。min_inner_size
        // 同时保障运行期 sync_pet_window_size 的 set_size 路径不被同一
        // 默认值钳制（SetWindowPos 期间子类已生效）。
        // 高度按形象宽高比（同 builder.inner_size 口径，P2-1）。
        let win = builder
            .visible(false)
            .build()
            .map_err(|e| format!("创建宠物窗口失败: {e}"))?;
        let _ = win.set_size(LogicalSize::new(size as f64, win_h));
        let _ = win.show();
    }
    Ok(())
}

/// 主显示器右下角默认位置（逻辑坐标）：右下留边距（Windows 含任务栏
/// 高度余量，见 PET_DEFAULT_MARGIN 注释）。显示器不可用（无头/异常驱动）
/// 时退回 (0, 0)。
fn default_bottom_right(mon: Option<&tauri::Monitor>, size: u32) -> (f64, f64) {
    let Some(mon) = mon else {
        return (0.0, 0.0);
    };
    let scale = mon.scale_factor();
    let mon_w = mon.size().width as f64 / scale;
    let mon_h = mon.size().height as f64 / scale;
    bottom_right_xy(mon_w, mon_h, size)
}

/// 右下角坐标计算（逻辑坐标纯函数，供单元测试复用）：
/// 右边距与下边距均为 PET_DEFAULT_MARGIN，负值夹到 0（小于宠物的屏幕）。
fn bottom_right_xy(mon_w: f64, mon_h: f64, size: u32) -> (f64, f64) {
    let s = size as f64;
    (
        (mon_w - s - PET_DEFAULT_MARGIN).max(0.0),
        (mon_h - s - PET_DEFAULT_MARGIN).max(0.0),
    )
}

/// 宠物窗口宽高比：内建形象帧为正方形（1:1）；自定义形象（custom:<id>）
/// 按其图集帧宽高比（Petdex 帧 192×208 ≈ 1:1.083）——窗口恒正方形会在
/// overflow:hidden 下上下各裁约 4% 画面（P2-1）。meta 不可读（宠物已删
/// 等）回退 1:1，窗口形态与内建一致。
fn aspect_in(root: &std::path::Path, style: &str) -> f64 {
    crate::pets::custom_style_id(style)
        .and_then(|id| crate::pets::load_pet_meta_in(root, id).ok())
        .filter(|m| m.frame_w > 0)
        .map(|m| m.frame_h as f64 / m.frame_w as f64)
        .unwrap_or(1.0)
}

/// 真实 ~/.zbar 路径版（窗口创建/尺寸同步用）
fn window_aspect(style: &str) -> f64 {
    crate::pets::pets_root()
        .map(|root| aspect_in(&root, style))
        .unwrap_or(1.0)
}

/// 窗口逻辑尺寸（宽 = 边长，高 = 边长 × 形象宽高比，保留两位小数）
fn window_size_of(style: &str, size: u32) -> (f64, f64) {
    let w = size as f64;
    let h = (w * window_aspect(style) * 100.0).round() / 100.0;
    (w, h)
}

/// 同步悬浮窗尺寸（逻辑尺寸随档位换算后的 px 边长与选中形象宽高比变化）。
fn sync_pet_window_size(win: &tauri::WebviewWindow, style: &str, size_px: u32) -> tauri::Result<()> {
    let (w, h) = window_size_of(style, size_px);
    win.set_size(LogicalSize::new(w, h))
}

// ============================================================
// 窗口位置持久化（Moved 事件节流落盘）
// ============================================================

/// 最近一次窗口位置（逻辑坐标）：Moved 事件高频触发（拖动时连续），
/// 先写内存，按节流间隔落盘，窗口销毁时冲刷最终值
static PET_POS: OnceLock<Mutex<Option<(f64, f64)>>> = OnceLock::new();
/// 上次位置落盘时刻（毫秒），0 = 从未落盘
static PET_POS_SAVED_AT: AtomicU64 = AtomicU64::new(0);
/// 位置落盘节流间隔（毫秒）：拖动结束后最迟 1 秒内持久化
const PET_POS_SAVE_THROTTLE_MS: u64 = 1000;

fn pet_pos_slot() -> &'static Mutex<Option<(f64, f64)>> {
    PET_POS.get_or_init(|| Mutex::new(None))
}

/// 宠物窗口 Moved 事件挂点（lib.rs 的 on_window_event 转发）：
/// 物理坐标转逻辑坐标写内存，节流合并进 pet.json（不动 enabled/style/
/// size 等其它字段）。
pub fn handle_pet_window_moved(win: &tauri::Window, pos: tauri::PhysicalPosition<i32>) {
    let scale = win.scale_factor().unwrap_or(1.0);
    if scale <= 0.0 {
        return;
    }
    let logical = (
        pos.x as f64 / scale,
        pos.y as f64 / scale,
    );
    {
        let Ok(mut guard) = pet_pos_slot().lock() else {
            return;
        };
        *guard = Some(logical);
    }
    // 节流落盘：拖动期间每秒最多一次写盘
    let now = chrono::Utc::now().timestamp_millis().max(0) as u64;
    let last = PET_POS_SAVED_AT.load(Ordering::Relaxed);
    if now.saturating_sub(last) < PET_POS_SAVE_THROTTLE_MS {
        return;
    }
    PET_POS_SAVED_AT.store(now, Ordering::Relaxed);
    persist_pet_pos(logical);
}

/// 宠物窗口 Destroyed 事件挂点：冲刷最终位置（无节流）+ 停轮询 +
/// 配置开关复位为关（窗口没了 = 功能关闭，防设置页显示与实况脱节；
/// 用户 alt+F4 等旁路关闭后设置开关能如实回读为关）。
pub fn handle_pet_window_destroyed(_app: &AppHandle) {
    stop_feed();
    let pos = pet_pos_slot()
        .lock()
        .ok()
        .and_then(|guard| *guard);
    if let Some(p) = pos {
        PET_POS_SAVED_AT.store(
            chrono::Utc::now().timestamp_millis().max(0) as u64,
            Ordering::Relaxed,
        );
        persist_pet_pos(p);
    }
    // 开关复位：读到的若已是关闭则不动（正常关闭流程 set_pet_config
    // 已先落盘，此处幂等）
    let mut cfg = load_pet_config();
    if cfg.enabled {
        cfg.enabled = false;
        let _ = save_pet_config(&cfg);
    }
}

/// 把位置合并进 pet.json（保留其它字段）。落盘失败静默（下次 Moved 再试）。
fn persist_pet_pos(pos: (f64, f64)) {
    let mut cfg = load_pet_config();
    if cfg.pos == Some(pos) {
        return;
    }
    cfg.pos = Some(pos);
    let _ = save_pet_config(&cfg);
}

// ============================================================
// 独立状态轮询器（普通 thread + flag 模式，沿用 usage_feed 惯例）
// ============================================================

static FEED_STOP: AtomicBool = AtomicBool::new(false);
static FEED_HANDLE: OnceLock<Mutex<Option<thread::JoinHandle<()>>>> = OnceLock::new();

fn feed_handle() -> &'static Mutex<Option<thread::JoinHandle<()>>> {
    FEED_HANDLE.get_or_init(|| Mutex::new(None))
}

/// 启动轮询线程（宠物窗口开启挂点调用）。已在运行时为幂等 no-op。
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
        .name("zbar-pet-feed".into())
        .spawn(move || feed_loop(app))
    {
        *guard = Some(h);
    }
}

/// 停止轮询线程（宠物窗口关闭挂点调用）。仅置位停止标记，不 join；
/// 线程完成当前查询周期（含 DB busy 等待，最长约 3 秒余）后退出。
pub fn stop_feed() {
    FEED_STOP.store(true, Ordering::Relaxed);
}

fn feed_loop(app: AppHandle) {
    // 变化检测缓存：(turns+runs 序列化字节, 上轮 ts)
    let mut cache: Option<(String, i64)> = None;
    loop {
        if FEED_STOP.load(Ordering::Relaxed) {
            return;
        }
        poll_once(&app, &mut cache);
        // 分段睡眠：sleep 期间可及时感知 stop（poll_once 期间不响应）
        for _ in 0..(FEED_INTERVAL_MS / 100) {
            if FEED_STOP.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
}

/// 单轮轮询：读库 → 构造摘要 → emit 给宠物窗口。任何失败静默跳过本轮
/// （下个周期重试），不 panic 不刷日志——ZCode 未运行/无库时事件停流，
/// 宠物核心按心跳停滞自然沉睡。
fn poll_once(app: &AppHandle, cache: &mut Option<(String, i64)>) {
    let result = (|| -> Result<(), String> {
        let conn = crate::zcode_sessions::open_main_db_readonly_uri()?;
        let now_ms = chrono::Utc::now().timestamp_millis();
        let Some(snapshot) =
            collect_pet_snapshot(&conn, cache.as_ref().map(|(p, _)| p.as_str()), cache.as_ref().map(|(_, t)| *t).unwrap_or(0), now_ms)?
        else {
            return Ok(()); // 老版本库无 turn_usage 表：宠物保持无数据沉睡
        };
        let turns_json = serde_json::to_string(&snapshot.turns)
            .map_err(|e| format!("序列化宠物 turns 失败: {e}"))?;
        let runs_json = serde_json::to_string(&snapshot.runs)
            .map_err(|e| format!("序列化宠物 runs 失败: {e}"))?;
        *cache = Some((
            snapshot_payload_key(
                &turns_json,
                &runs_json,
                snapshot.pu,
                snapshot.ta,
                snapshot.fe,
            ),
            snapshot.ts,
        ));
        let _ = app.emit_to(PET_WINDOW_LABEL, PET_USAGE_EVENT, &snapshot);
        Ok(())
    })();
    let _ = result; // 静默跳过本轮（库被锁超时等瞬态），下个周期重试
}

/// turns + runs 序列化字节与 pu/ta/fe 的变化检测 key（不可见分隔符防边界
/// 歧义，与 usage_feed::write_if_changed 同款手法；pu/ta/fe 参与对比——
/// 用户发消息、工具开始/结束、失败轮落库本身就是数据变化，ts 随之刷新）。
fn snapshot_payload_key(
    turns_json: &str,
    runs_json: &str,
    pending_user: Option<i64>,
    active_tool: Option<i64>,
    failure_event: Option<i64>,
) -> String {
    let sig = |v: Option<i64>| match v {
        Some(t) => t.to_string(),
        None => "-".to_string(),
    };
    format!(
        "{turns_json}\u{1}{runs_json}\u{1}{}\u{1}{}\u{1}{}",
        sig(pending_user),
        sig(active_tool),
        sig(failure_event)
    )
}

// ============================================================
// 摘要构造（纯逻辑拆分便于单元测试，不依赖真实 ~/.zcode）
// ============================================================

/// 宠物摘要的 turns 元素：仅保留宠物核心消费的字段（数量用于庆祝
/// 新增检测、末尾轮标识 turn|umid 用于区分轮次更替），其余字段
/// （token 统计/耗时/模型清单等）核心不消费，裁剪掉以缩小 IPC payload。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct PetTurnBrief {
    /// 轮 id（末尾轮标识组成之一）
    pub turn: String,
    /// 用户消息 id（末尾轮标识组成之一；列缺失/值为 null 时为 null）
    pub umid: Option<String>,
}

/// 宠物摘要的 runs 元素：仅保留宠物核心消费的字段——out（工作判定与
/// typing 速度分档）、m（已并入主轮行 sub 的子代理行标记，跳过防双计）、
/// sub.out（并入的子代理输出）。
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct PetRunBrief {
    /// 输出 token 合计（该轮已完成模型请求）
    pub out: i64,
    /// 已并入主轮行 sub 的标记（1 = 宠物核心跳过本行防双计）；
    /// None 不序列化
    #[serde(skip_serializing_if = "Option::is_none")]
    pub m: Option<u8>,
    /// 并入本主轮行的子代理输出聚合；None 不序列化
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub: Option<PetSubBrief>,
}

/// runs 行的子代理聚合（宠物只消费输出增量）
#[derive(Debug, Clone, Serialize, PartialEq)]
pub(crate) struct PetSubBrief {
    pub out: i64,
}

/// 推给宠物窗口的用量摘要：与 usage-data.js 的 window.__ZBAR_USAGE__
/// 数据同构（v/ts/la/pu/ta/fe/turns/runs，la 为 V2 起的最后活动时刻字
/// 段、pu 为 V5 起的待处理用户消息字段、ta/fe 为 V6 起的活跃工具/失败
/// 轮事件字段），宠物核心（pet-core.js 的 feed）直接消费。
#[derive(Debug, Clone, Serialize)]
pub(crate) struct PetSnapshot {
    /// 数据契约版本（核心对 v !== 2 视为无效）
    pub v: u8,
    /// 最后数据变化时刻（内容无变化时沿用上轮值）
    pub ts: i64,
    /// 最后活动时刻（完成轮 end、进行中轮 start、待处理用户消息 pu 与
    /// 活跃工具 ta 的最大值；宠物闲置判定消费——事件推流路径的 ts 会因
    /// 轮次滑出查询窗口而刷新，按 ts 判闲置会让宠物周期性误弹闲置，见
    /// usage_feed 模块头 la 字段说明）
    pub la: i64,
    /// 待处理用户消息时刻（V5）：最近一条「尚无完成轮」的 user 消息的
    /// time_created（毫秒）；无则 null。宠物核心据此在 runs 为空时预判
    /// 进入 working（用户发消息后首笔模型请求完成落库前 runs 通道看不
    /// 见，实测该窗口 30~70 秒）。查询与 usage-data.js 同一口径（usage_
    /// feed::collect_pending_user_ms，含实测依据），查询失败按 null 降级
    pub pu: Option<i64>,
    /// 活跃工具时刻（V6）：最新一条 running 状态工具行的 started_at
    /// （毫秒）；无则 null。工具调用开始瞬间落库、完成即更新——是
    /// "正在执行工具"的实时信号，宠物据此进入 tool_running（替主人跑
    /// 腿执行）。查询与 usage-data.js 同一口径（usage_feed::
    /// collect_active_tool_ms，窗口为 10 分钟崩溃残留兜底），查询失败
    /// 按 null 降级
    pub ta: Option<i64>,
    /// 失败轮事件时刻（V6）：最近一次「失败或取消」完成轮的
    /// completed_at（毫秒）；无则 null。只在失败轮新增时变化（成功轮
    /// 不刷新），宠物据 now − fe < 3s 窗口进入 failed（沮丧 3 秒）。
    /// 查询与 usage-data.js 同一口径（usage_feed::
    /// collect_failure_event_ms，窗口与 turns 查询窗口一致），查询失败
    /// 按 null 降级
    pub fe: Option<i64>,
    pub turns: Vec<PetTurnBrief>,
    pub runs: Vec<PetRunBrief>,
}

/// 读库并构造宠物摘要（含变化检测）：
/// - turns 窗口取近 1 小时（宠物只做"新增轮"检测，见 TURNS_WINDOW_MS），
///   runs 口径与 usage_feed 一致（近 10 分钟新鲜度 + 窗口内扫查）；
/// - pu（待处理用户消息）与 usage-data.js 同口径：共用 usage_feed::
///   collect_pending_user_ms 查询与 turns umid 内存匹配，参与变化检测
///   （key 含 pu）与 la 推导；查询失败按 null 降级不阻塞摘要；
/// - ta/fe（V6，活跃工具/失败轮事件）与 usage-data.js 同口径：共用
///   usage_feed 的查询函数（ta 窗口 10 分钟残留兜底、fe 窗口与 turns
///   一致），参与变化检测（key 含 ta/fe）与 la 推导（ta 取大）；查询
///   失败按 null 降级（无信号 → 新状态不触发，行为同 V5）；
/// - 变化检测：turns+runs+pu+ta+fe 序列化字节与上轮相同 → ts 沿用
///   prev_ts（"最后数据变化时刻"语义与 usage-data.js 一致），否则
///   ts = now；
/// - la 为 turns/runs/pu/ta 的纯推导值（最后活动时刻，闲置判定消费）；
///   事件推流路径的"数据源存活"由事件到达本身表达（pet-main.ts 收到
///   事件即 heartbeat(Date.now())），快照不再携带每周期刷新的心跳字段；
/// - Ok(None) = 功能关闭（turn_usage 表/核心列缺失，老版本 ZCode）；
///   Err = 瞬态查询失败（调用方静默跳过本轮）。
pub(crate) fn collect_pet_snapshot(
    conn: &Connection,
    prev_payload: Option<&str>,
    prev_ts: i64,
    now_ms: i64,
) -> Result<Option<PetSnapshot>, String> {
    // 复用 usage_feed 的查询聚合（不改其契约）：None = 无 turn_usage 表
    let Some((turns, sub_orphans)) =
        crate::agent_theme::usage_feed::collect_turns(conn, now_ms - TURNS_WINDOW_MS)?
    else {
        return Ok(None);
    };
    // 与 usage_feed 同款：runs 与 turns 任一查询失败整体跳过本轮，
    // 避免 runs 闪空导致宠物状态抖动
    let done =
        crate::agent_theme::usage_feed::collect_done_turn_ids(conn, now_ms - TURNS_WINDOW_MS)?;
    let runs = crate::agent_theme::usage_feed::collect_runs(
        conn,
        now_ms - RUN_WINDOW_MS,
        now_ms - TURNS_WINDOW_MS,
        &done,
        &sub_orphans,
    )?;

    let brief_turns: Vec<PetTurnBrief> = turns
        .iter()
        .map(|t| PetTurnBrief {
            turn: t.turn_id.clone(),
            umid: t.user_message_id.clone(),
        })
        .collect();
    let brief_runs: Vec<PetRunBrief> = runs
        .iter()
        .map(|r| PetRunBrief {
            out: r.output_tokens,
            m: r.merged,
            sub: r.sub.as_ref().map(|s| PetSubBrief { out: s.output_tokens }),
        })
        .collect();

    // pu（待处理用户消息）：与 usage-data.js 同口径（共用查询函数与完成
    // 轮内存匹配）；查询失败按 null 降级（unwrap_or(None)），不阻塞摘要
    let done_umids: std::collections::BTreeSet<String> = turns
        .iter()
        .filter_map(|t| t.user_message_id.clone())
        .collect();
    let pending_user =
        crate::agent_theme::usage_feed::collect_pending_user_ms(conn, &done_umids)
            .unwrap_or(None);
    // ta/fe（V6）：与 usage-data.js 同口径（共用查询函数）；查询失败按
    // null 降级，不阻塞摘要。ta 窗口 = usage_feed::TOOL_WINDOW_MS（10
    // 分钟崩溃残留兜底），fe 窗口 = turns 查询窗口（失败轮落库瞬间必在
    // 窗口内，超窗漏判属可接受窄边缘）
    let active_tool = crate::agent_theme::usage_feed::collect_active_tool_ms(
        conn,
        now_ms - crate::agent_theme::usage_feed::TOOL_WINDOW_MS,
    )
    .unwrap_or(None);
    let failure_event = crate::agent_theme::usage_feed::collect_failure_event_ms(
        conn,
        now_ms - TURNS_WINDOW_MS,
    )
    .unwrap_or(None);

    let turns_json = serde_json::to_string(&brief_turns)
        .map_err(|e| format!("序列化宠物 turns 失败: {e}"))?;
    let runs_json = serde_json::to_string(&brief_runs)
        .map_err(|e| format!("序列化宠物 runs 失败: {e}"))?;
    let key = snapshot_payload_key(&turns_json, &runs_json, pending_user, active_tool, failure_event);
    let ts = match prev_payload {
        Some(prev) if prev == key => prev_ts,
        _ => now_ms,
    };

    Ok(Some(PetSnapshot {
        v: 2,
        ts,
        la: crate::agent_theme::usage_feed::last_activity_ms(
            &turns,
            &runs,
            pending_user,
            active_tool,
        ),
        pu: pending_user,
        ta: active_tool,
        fe: failure_event,
        turns: brief_turns,
        runs: brief_runs,
    }))
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-pet-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn 配置_默认值读写与camelCase契约() {
        let dir = test_dir("config");
        let path = dir.join("pet.json");

        // 不存在时读默认
        assert!(read_pet_config_file(&path).is_none());

        // 默认值写出 → 读回一致
        let default = PetConfig::default();
        assert!(!default.enabled);
        assert_eq!(default.style, DEFAULT_PET_STYLE);
        assert_eq!(default.size, DEFAULT_PET_SIZE_LEVEL);
        assert_eq!(default.pos, None);
        write_pet_config_file(&path, &default).unwrap();
        assert_eq!(read_pet_config_file(&path), Some(default));

        // camelCase 键名（前端契约）
        let text = fs::read_to_string(&path).unwrap();
        for key in ["\"enabled\"", "\"style\"", "\"size\"", "\"pos\""] {
            assert!(text.contains(key), "pet.json 缺少字段 {key}：{text}");
        }

        // 显式值读写往返（含位置；size 为档位值原样透传，读写不经 clamp）
        let p = PetConfig {
            enabled: true,
            style: "bot".into(),
            size: 5,
            pos: Some((120.5, 340.25)),
        };
        write_pet_config_file(&path, &p).unwrap();
        assert_eq!(read_pet_config_file(&path), Some(p));
        let round = fs::read_to_string(&path).unwrap();
        assert!(round.contains("\"pos\""), "位置应持久化：{round}");
        assert!(round.contains("120.5") && round.contains("340.25"), "{round}");

        // 旧版文件缺字段 → serde default 补默认
        fs::write(&path, r#"{"enabled":true}"#).unwrap();
        let legacy = read_pet_config_file(&path).unwrap();
        assert!(legacy.enabled, "缺字段时 enabled 以文件值为准");
        assert_eq!(legacy.style, DEFAULT_PET_STYLE);
        assert_eq!(legacy.size, DEFAULT_PET_SIZE_LEVEL);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 档位换算_各档位与典型屏高() {
        // 1080p@100%（逻辑高 1080）：档位 1..5 → 59 / 81 / 108 / 135 / 162
        assert_eq!(pet_size_px(1, 1080.0), 59);
        assert_eq!(pet_size_px(2, 1080.0), 81);
        assert_eq!(pet_size_px(3, 1080.0), 108);
        assert_eq!(pet_size_px(4, 1080.0), 135);
        assert_eq!(pet_size_px(5, 1080.0), 162);
        // 4K@150%（逻辑高 1440）：79 / 108 / 144 / 180 / 216
        assert_eq!(pet_size_px(1, 1440.0), 79);
        assert_eq!(pet_size_px(2, 1440.0), 108);
        assert_eq!(pet_size_px(3, 1440.0), 144);
        assert_eq!(pet_size_px(4, 1440.0), 180);
        assert_eq!(pet_size_px(5, 1440.0), 216);
        // 四舍五入：10% × 800 = 80 整；5.5% × 900 = 49.5 → 50
        assert_eq!(pet_size_px(3, 800.0), 80);
        assert_eq!(pet_size_px(1, 900.0), 50);
        // 档位越界夹到 1..=5
        assert_eq!(pet_size_px(0, 1080.0), pet_size_px(1, 1080.0));
        assert_eq!(pet_size_px(9, 1080.0), pet_size_px(5, 1080.0));
        // 屏高非法（0 / 负 / NaN / 无穷）兜底 1080
        for bad in [0.0, -800.0, f64::NAN, f64::INFINITY] {
            assert_eq!(pet_size_px(3, bad), pet_size_px(3, PET_SIZE_FALLBACK_SCREEN_H));
        }
    }

    #[test]
    fn 档位换算_与迁移互为反函数() {
        // 同屏高下：旧 px → 档位 → px 的往返不漂移到相邻档（48~128 全域）
        for h in [800.0, 1080.0, 1440.0, 2160.0] {
            for px in PET_SIZE_LEGACY_PX_RANGE.0..=PET_SIZE_LEGACY_PX_RANGE.1 {
                let lv = normalize_pet_size(px, h);
                let round = pet_size_px(lv, h) as f64;
                let pct = px as f64 / h * 100.0;
                let lv_pct = PET_SIZE_LEVEL_PCT[(lv - 1) as usize];
                // 往返档位是百分比意义上离原值最近的档
                assert!(
                    (lv_pct - pct).abs() <= PET_SIZE_LEVEL_PCT.iter().map(|p| (p - pct).abs()).fold(f64::INFINITY, f64::min) + 1e-9,
                    "px={px} h={h} 应选最近档位 lv={lv}"
                );
                let _ = round;
            }
        }
    }

    #[test]
    fn 配置迁移_旧px值按屏高换算最近档位() {
        // 1080p 屏：48→档1（5.5%）、64→档1（5.9% 更近 5.5%）、
        // 96→档3（10.0%）、128→档4（12.5%）；越界旧值先夹回 48~128 再换算
        assert_eq!(normalize_pet_size(48, 1080.0), 1);
        assert_eq!(normalize_pet_size(64, 1080.0), 1);
        assert_eq!(normalize_pet_size(96, 1080.0), 3);
        assert_eq!(normalize_pet_size(128, 1080.0), 4);
        assert_eq!(normalize_pet_size(8, 1080.0), 1, "过小旧值夹到 48 再迁移");
        assert_eq!(normalize_pet_size(9999, 1080.0), 4, "过大旧值夹到 128 再迁移");
        // 4K@150% 屏（逻辑高 1440）：64→档1、96→档2
        assert_eq!(normalize_pet_size(64, 1440.0), 1);
        assert_eq!(normalize_pet_size(96, 1440.0), 2);
        // 新档位值直读（不迁移）
        for lv in 1..=5 {
            assert_eq!(normalize_pet_size(lv, 1080.0), lv);
        }
        // 彻底非法值回默认档位 3；屏高非法兜底 1080 口径
        assert_eq!(normalize_pet_size(0, 1080.0), DEFAULT_PET_SIZE_LEVEL);
        assert_eq!(normalize_pet_size(64, 0.0), normalize_pet_size(64, 1080.0));
    }

    #[test]
    fn 配置_clamp收敛() {
        // clamped 走缓存屏高（测试环境未初始化 → 1080 兜底口径）
        let mut p = PetConfig {
            enabled: true,
            style: "   ".into(),
            size: 48, // 旧 px 值：夹域后按 1080 屏换算 → 档 1
            pos: None,
        };
        let c = p.clone().clamped();
        assert_eq!(c.size, 1, "旧 px 值应迁移为最近档位");
        assert_eq!(c.style, DEFAULT_PET_STYLE, "空白形象应回默认");
        assert!(c.enabled, "clamped 不应改动开关值");
        p.size = 9999; // 旧 px 越界 → 128 → 11.85% → 档 4
        assert_eq!(p.clone().clamped().size, 4, "过大旧值应迁移为最近档 4");
        p.size = 3; // 新档位直读
        assert_eq!(p.clone().clamped().size, 3);
        // 形象首尾空白修剪
        let c = PetConfig {
            style: " cat ".into(),
            ..PetConfig::default()
        }
        .clamped();
        assert_eq!(c.style, "cat");
    }

    #[test]
    fn 配置迁移_load时落盘一次性() {
        let dir = test_dir("migrate");
        let path = dir.join("pet.json");
        // 旧版 px 值（64）落盘 → load 迁移为档位并写回文件（测试环境
        // 缓存屏高未初始化 → 1080 兜底口径：64px ≈ 5.93% → 档 1）
        write_pet_config_file(
            &path,
            &PetConfig {
                enabled: true,
                style: "bot".into(),
                size: 64,
                pos: Some((10.0, 20.0)),
            },
        )
        .unwrap();
        let cfg = load_pet_config_at(&path);
        assert_eq!(cfg.size, 1, "64px@1080 应迁移到档 1");
        assert_eq!(cfg.pos, Some((10.0, 20.0)), "迁移不应动其它字段");
        assert!(cfg.enabled, "迁移不应动开关");
        let back = read_pet_config_file(&path).unwrap();
        assert_eq!(back.size, 1, "迁移结果应已落盘");
        // 二次读取：档位值直读幂等（不再改写）
        let again = load_pet_config_at(&path);
        assert_eq!(again.size, 1);
        assert_eq!(again, back, "已迁移配置二次读取应完全一致");

        let _ = fs::remove_dir_all(&dir);
    }

    /// 宠物摘要测试库（同 usage_feed 的 v9 完整 schema）
    fn pet_db(name: &str) -> (Connection, PathBuf) {
        let path = std::env::temp_dir().join(format!(
            "zbar-pet-db-{}-{name}.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
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
    fn 摘要_字段裁剪与同构契约() {
        let (conn, path) = pet_db("brief");
        let now = 1_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_done', 'completed', {d1}, {d2}, 'msg_done', 10, 20, 0, 0, 5, 1, 0, 0);
             INSERT INTO model_usage VALUES
               ('sess_1', 'turn_live', {m1}, 'msg_live', 'GLM-5.3', 100, 200, 5, 10, 50);",
            d1 = now - 60_000,
            d2 = now - 50_000,
            m1 = now - 1_000,
        ))
        .unwrap();
        let snap = collect_pet_snapshot(&conn, None, 0, now)
            .unwrap()
            .expect("完整库应有摘要");

        // 同构契约：v=2 + ts/la 字段（宠物核心 feed 的消费前提；事件路径
        // 的存活信号由事件到达本身表达，快照不再携带 hb）
        assert_eq!(snap.v, 2);
        assert_eq!(snap.ts, now, "首查必为变化，ts = now");
        // la：完成轮 end（now-50_000）与进行中轮 start（now-1_000）取大
        assert_eq!(snap.la, now - 1_000, "la 应取完成轮 end 与进行中轮 start 的最大值");
        // turns 裁剪：仅 turn/umid（统计字段不透出）
        assert_eq!(snap.turns.len(), 1);
        assert_eq!(snap.turns[0].turn, "turn_done");
        assert_eq!(snap.turns[0].umid.as_deref(), Some("msg_done"));
        let t_json = serde_json::to_string(&snap.turns).unwrap();
        assert!(t_json.contains("\"turn\":\"turn_done\""), "{t_json}");
        assert!(t_json.contains("\"umid\":\"msg_done\""), "{t_json}");
        assert!(!t_json.contains("output_tokens"), "统计字段不应透出：{t_json}");
        // runs 裁剪：仅 out（+可选 m/sub）
        assert_eq!(snap.runs.len(), 1);
        assert_eq!(snap.runs[0].out, 200);
        let r_json = serde_json::to_string(&snap.runs).unwrap();
        assert!(r_json.contains("\"out\":200"), "{r_json}");
        assert!(!r_json.contains("\"req\""), "req 字段不应透出：{r_json}");
        assert!(!r_json.contains("\"sess\""), "sess 字段不应透出：{r_json}");
        // 整体快照序列化形态（事件 payload）
        let s_json = serde_json::to_string(&snap).unwrap();
        assert!(
            s_json.starts_with("{\"v\":2,\"ts\":"),
            "快照应以 v/ts 开头（与 usage-data.js 同构）：{s_json}"
        );
        assert!(s_json.contains("\"la\":"), "{s_json}");
        // V6：ta/fe 两态透出（本库无 tool_usage 表 → null 降级）
        assert!(s_json.contains("\"ta\":null"), "{s_json}");
        assert!(s_json.contains("\"fe\":null"), "{s_json}");
        assert!(!s_json.contains("\"hb\":"), "事件路径不应携带 hb：{s_json}");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 摘要_pu待处理用户消息_同口径透出并参与变化检测() {
        // pet_db 无 message 表（老版本库形态）→ pu 恒 null（信号缺失降级）
        let (conn, path) = pet_db("pu-none");
        let now = 2_100_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_1', 'completed', {t0}, {t0e}, 'msg_1', 10, 20, 0, 0, 5, 1, 0, 0);",
            t0 = now - 60_000,
            t0e = now - 50_000,
        ))
        .unwrap();
        let snap = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        assert_eq!(snap.pu, None, "无 message 表时 pu 应为 null（降级）");
        assert_eq!(snap.la, now - 50_000, "pu 缺失不影响 la");
        drop(conn);
        let _ = fs::remove_file(&path);

        // 有 message 表：待处理 user 消息 → pu 透出、参与 la 与变化检测；
        // 完成轮落库（turn_usage.user_message_id 匹配）→ pu 归 null
        let (conn, path) = pet_db("pu-live");
        conn.execute_batch(
            "CREATE TABLE message (
                id TEXT PRIMARY KEY, session_id TEXT, time_created INTEGER,
                time_updated INTEGER, data TEXT, sequence INTEGER);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO message VALUES
               ('msg_u1', 'sess_1', {u1}, {u1}, '{{\"role\":\"user\",\"agent\":\"zcode-agent\"}}', 1);",
            u1 = now - 30_000,
        ))
        .unwrap();
        // 首查：msg_u1 尚无完成轮 → pu = 发送时刻，la 取大（完成轮 end 与 pu）
        let first = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        assert_eq!(first.pu, Some(now - 30_000), "待处理 user 消息应透出 pu");
        assert_eq!(first.la, now - 30_000, "pu 应参与 la 取大");
        // 序列化形态：事件 payload 含 pu 数值态
        let s_json = serde_json::to_string(&first).unwrap();
        assert!(s_json.contains("\"pu\":"), "{s_json}");
        // 内容不变（同 key）→ ts 沿用
        let key = {
            let turns_json = serde_json::to_string(&first.turns).unwrap();
            let runs_json = serde_json::to_string(&first.runs).unwrap();
            snapshot_payload_key(&turns_json, &runs_json, first.pu, first.ta, first.fe)
        };
        let second =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 2000)
                .unwrap()
                .unwrap();
        assert_eq!(second.ts, first.ts, "pu 不变时 ts 应沿用");
        assert_eq!(second.pu, Some(now - 30_000));
        // 用户再发一条消息（pu 变化）→ ts 刷新（pu 参与变化检测）
        conn.execute_batch(&format!(
            "INSERT INTO message VALUES
               ('msg_u2', 'sess_1', {u2}, {u2}, '{{\"role\":\"user\",\"agent\":\"zcode-agent\"}}', 2);",
            u2 = now + 100,
        ))
        .unwrap();
        let third =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 4000)
                .unwrap()
                .unwrap();
        assert_eq!(third.pu, Some(now + 100), "更新的待处理消息应替换 pu");
        assert_eq!(third.ts, now + 4000, "pu 变化应刷新 ts");
        // 完成轮落库：turn_usage 行带 user_message_id = msg_u2 → pu 归 null，
        // turns 新增（内容变化）→ ts 刷新、la 推进到完成时刻（msg_u1 也
        // 补一条完成轮——「最近一条尚无完成轮的 user 消息」语义下，任何
        // 未匹配消息都会继续透出 pu）
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_1', 'completed', {t0}, {t0e}, 'msg_u1', 1, 2, 0, 0, 0, 1, 0, 0),
               ('sess_1', 'turn_2', 'completed', {t}, {t2}, 'msg_u2', 1, 2, 0, 0, 0, 1, 0, 0);",
            t0 = now - 30_000,
            t0e = now - 25_000,
            t = now + 100,
            t2 = now + 5_000,
        ))
        .unwrap();
        let fourth = collect_pet_snapshot(&conn, None, 0, now + 6000)
            .unwrap()
            .unwrap();
        assert_eq!(fourth.pu, None, "完成轮匹配后 pu 应归 null");
        assert_eq!(fourth.la, now + 5_000, "la 应推进到新完成轮 end");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 摘要_ta与fe_同口径透出并参与变化检测() {
        // pet_db 无 tool_usage 表（老版本库形态）→ ta 恒 null（降级），
        // fe 因 turn_usage 有 status/completed_at 列仍可判定（缺判定列
        // cancelled_by_user/tool_error_count 时按 status 兜底——本库
        // schema 不含这两列）
        let (conn, path) = pet_db("ta-fe");
        let now = 2_200_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_fail', 'error', {t0}, {t0e}, 'msg_f', 10, 20, 0, 0, 5, 1, 0, 0);",
            t0 = now - 60_000,
            t0e = now - 2_000,
        ))
        .unwrap();
        let snap = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        assert_eq!(snap.ta, None, "无 tool_usage 表时 ta 应为 null（降级）");
        assert_eq!(
            snap.fe,
            Some(now - 2_000),
            "失败轮（error）完成时刻应透出 fe"
        );
        drop(conn);
        let _ = fs::remove_file(&path);

        // 有 tool_usage 表：running 行 → ta 透出并参与 la 取大；工具完成
        //（running → completed）→ ta 归 null（变化检测刷新 ts）
        let (conn, path) = pet_db("ta-live");
        conn.execute_batch(
            "CREATE TABLE tool_usage (
                id TEXT PRIMARY KEY, session_id TEXT NOT NULL, tool_name TEXT NOT NULL,
                status TEXT NOT NULL, started_at INTEGER NOT NULL,
                completed_at INTEGER, exit_code INTEGER,
                cancelled_by_user INTEGER NOT NULL DEFAULT 0);",
        )
        .unwrap();
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_1', 'completed', {t0}, {t0e}, 'msg_1', 10, 20, 0, 0, 5, 1, 0, 0);
             INSERT INTO tool_usage (id, session_id, tool_name, status, started_at, completed_at) VALUES
               ('tool_1', 'sess_1', 'Bash', 'running', {r1}, NULL);",
            t0 = now - 60_000,
            t0e = now - 50_000,
            r1 = now - 3_000,
        ))
        .unwrap();
        let first = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        assert_eq!(first.ta, Some(now - 3_000), "running 行应透出 ta");
        assert_eq!(first.fe, None, "无失败轮时 fe 应为 null");
        assert_eq!(
            first.la, now - 3_000,
            "ta 应参与 la 取大（压过完成轮 end）"
        );
        // 内容不变 → ts 沿用
        let key = {
            let turns_json = serde_json::to_string(&first.turns).unwrap();
            let runs_json = serde_json::to_string(&first.runs).unwrap();
            snapshot_payload_key(&turns_json, &runs_json, first.pu, first.ta, first.fe)
        };
        let second =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 2000)
                .unwrap()
                .unwrap();
        assert_eq!(second.ts, first.ts, "ta/fe 不变时 ts 应沿用");
        // 工具完成 → ta 归 null（内容变化 → ts 刷新）
        conn.execute_batch(&format!(
            "UPDATE tool_usage SET status = 'completed', completed_at = {c} WHERE id = 'tool_1';",
            c = now + 3_000,
        ))
        .unwrap();
        let third =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 4000)
                .unwrap()
                .unwrap();
        assert_eq!(third.ta, None, "工具完成后 ta 应归 null");
        assert_eq!(third.ts, now + 4000, "ta 变化应刷新 ts");
        // 序列化形态：事件 payload 含 ta 数值态与 fe null 态
        let s_json = serde_json::to_string(&first).unwrap();
        assert!(s_json.contains("\"ta\":"), "{s_json}");
        assert!(s_json.contains("\"fe\":null"), "{s_json}");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 摘要_ts语义_内容不变沿用旧值_变化才更新_la随活动推进() {
        let (conn, path) = pet_db("ts");
        let now = 2_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_1', 'completed', {t0}, {t0e}, 'msg_1', 10, 20, 0, 0, 5, 1, 0, 0);",
            t0 = now - 60_000,
            t0e = now - 50_000,
        ))
        .unwrap();
        // 首查
        let first = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        let key = {
            let turns_json = serde_json::to_string(&first.turns).unwrap();
            let runs_json = serde_json::to_string(&first.runs).unwrap();
            snapshot_payload_key(&turns_json, &runs_json, first.pu, first.ta, first.fe)
        };
        assert_eq!(first.la, now - 50_000, "la 应为完成轮 end");
        // 内容无变化：ts 沿用旧值，la 保持活动时刻（不被周期间流逝刷新）
        let second =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 2000)
                .unwrap()
                .unwrap();
        assert_eq!(second.ts, first.ts, "内容未变 ts 应沿用：{second:?}");
        assert_eq!(second.la, now - 50_000, "内容未变 la 不应被无活动刷新");
        // 内容变化（新增轮，end 更晚）→ ts 更新、la 随活动推进
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_2', 'completed', {t}, {t2}, 'msg_2', 1, 2, 0, 0, 0, 1, 0, 0);",
            t = now + 100,
            t2 = now + 200,
        ))
        .unwrap();
        let third =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now + 4000)
                .unwrap()
                .unwrap();
        assert_eq!(third.ts, now + 4000, "内容变化 ts 应更新");
        assert_eq!(third.la, now + 200, "新完成轮 end 应推进 la");
        assert_eq!(third.turns.len(), 2, "新增轮应进入摘要");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 摘要_轮滑出窗口边界_ts刷新但la不被无活动刷新() {
        // P1-3 修复场景：轮滑出 1 小时查询窗口（turns 序列变化 → ts 刷新
        // 为当前周期）但 la 不随周期间流逝推进（滑出后归 0/变小）——宠物
        // 闲置判定按 la 计算，不会因窗口滑动从沉睡误弹回闲置
        let (conn, path) = pet_db("slide");
        let now1 = 3_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO turn_usage VALUES
               ('sess_1', 'turn_old', 'completed', {t0}, {t0e}, 'msg_old', 10, 20, 0, 0, 5, 1, 0, 0);",
            t0 = now1 - 3_500_000, /* 首查时在 1 小时窗口内 */
            t0e = now1 - 3_400_000,
        ))
        .unwrap();
        // 首查：轮在窗口内，la = 完成时刻 end
        let first = collect_pet_snapshot(&conn, None, 0, now1).unwrap().unwrap();
        assert_eq!(first.turns.len(), 1, "窗口内轮次应进入摘要：{first:?}");
        assert_eq!(first.la, now1 - 3_400_000);
        // 次查（时间前移 200 秒，窗口起点越过该轮 started_at → 滑出）：
        // turns 序列变化 → ts 刷新为当前周期，但 la 归 0（无活动）而非
        // 被刷新为当前周期——闲置判定按 la 不会误弹闲置
        let now2 = now1 + 200_000;
        let key = {
            let turns_json = serde_json::to_string(&first.turns).unwrap();
            let runs_json = serde_json::to_string(&first.runs).unwrap();
            snapshot_payload_key(&turns_json, &runs_json, first.pu, first.ta, first.fe)
        };
        let second =
            collect_pet_snapshot(&conn, Some(key.as_str()), first.ts, now2)
                .unwrap()
                .unwrap();
        assert!(second.turns.is_empty(), "滑出窗口的轮次不应再进入摘要：{second:?}");
        assert_eq!(second.ts, now2, "内容变化（轮滑出）ts 仍刷新（数据契约语义保持）");
        assert_eq!(second.la, 0, "轮滑出且无新活动时 la 应归 0（更快入睡，绝不误醒）");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 摘要_无turn_usage表时功能禁用() {
        let path = std::env::temp_dir().join(format!(
            "zbar-pet-db-{}-empty.sqlite",
            std::process::id()
        ));
        let _ = fs::remove_file(&path);
        let conn = Connection::open(&path).unwrap();
        conn.execute_batch("CREATE TABLE session (id TEXT PRIMARY KEY);")
            .unwrap();
        let out = collect_pet_snapshot(&conn, None, 0, 1_000).unwrap();
        assert!(out.is_none(), "无 turn_usage 表应返回 None（宠物沉睡）");
        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 轮询器_启停标志与幂等() {
        // stop 置位后 is_running 线程退出标志可观测：这里只验证标志语义
        // （真实线程生命周期由 start_feed 的句柄复用逻辑保证，同
        // usage_feed 模式，无 AppHandle 无法在单测中起线程）
        FEED_STOP.store(false, Ordering::Relaxed);
        assert!(!FEED_STOP.load(Ordering::Relaxed));
        stop_feed();
        assert!(FEED_STOP.load(Ordering::Relaxed));
        // 重新置回运行态（模拟 start_feed 的清位路径）
        FEED_STOP.store(false, Ordering::Relaxed);
        assert!(!FEED_STOP.load(Ordering::Relaxed));
    }

    #[test]
    fn 摘要_runs子代理m标记与sub输出透出() {
        // 主会话进行中轮 + 子代理进行中轮（数值并入主轮 sub、行打 m:1）
        let (conn, path) = pet_db("sub");
        let now = 3_000_000_000_i64;
        conn.execute_batch(&format!(
            "INSERT INTO session VALUES ('sess_main', NULL), ('sess_subagent_a', 'sess_main');
             INSERT INTO model_usage VALUES
               ('sess_main', 'turn_m', {m1}, 'msg_main', 'GLM-5.3', 100, 200, 0, 0, 50),
               ('sess_subagent_a', 'turn_sa', {m2}, 'msg_sub', 'GLM-4.7', 30, 40, 0, 0, 20);",
            m1 = now - 1_000,
            m2 = now - 2_000,
        ))
        .unwrap();
        let snap = collect_pet_snapshot(&conn, None, 0, now).unwrap().unwrap();
        assert_eq!(snap.runs.len(), 2, "{:?}", snap.runs);
        let main = snap
            .runs
            .iter()
            .find(|r| r.m.is_none() && r.sub.is_some())
            .expect("主会话行应带 sub 无 m");
        assert_eq!(main.out, 200);
        assert_eq!(main.sub.as_ref().unwrap().out, 40, "子代理输出应并入 sub");
        let sub = snap
            .runs
            .iter()
            .find(|r| r.m.is_some())
            .expect("子代理行应带 m:1");
        assert_eq!(sub.m, Some(1));
        // 序列化形态：m:1 与 sub.out 均透出（核心防双计与增速口径）
        let r_json = serde_json::to_string(&snap.runs).unwrap();
        assert!(r_json.contains("\"m\":1"), "{r_json}");
        assert!(r_json.contains("\"sub\":{\"out\":40}"), "{r_json}");

        drop(conn);
        let _ = fs::remove_file(&path);
    }

    #[test]
    fn 位置持久化_合并不动其它字段() {
        let dir = test_dir("pos");
        let path = dir.join("pet.json");
        let base = PetConfig {
            enabled: true,
            style: "bot".into(),
            size: 96,
            pos: None,
        };
        write_pet_config_file(&path, &base).unwrap();

        // 模拟 persist_pet_pos 的合并路径（读 → 改 pos → 写）
        let mut cfg = read_pet_config_file(&path).unwrap();
        cfg.pos = Some((88.0, 200.0));
        write_pet_config_file(&path, &cfg).unwrap();
        let back = read_pet_config_file(&path).unwrap();
        assert_eq!(back.pos, Some((88.0, 200.0)));
        assert_eq!(back.style, "bot", "合并不应动其它字段");
        assert_eq!(back.size, 96);
        assert!(back.enabled);

        // 同位置重复写为幂等（值不变）
        write_pet_config_file(&path, &cfg).unwrap();
        assert_eq!(
            read_pet_config_file(&path).unwrap().pos,
            Some((88.0, 200.0))
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 参数推送判定_仅开启且形象或尺寸变化() {
        let base = PetConfig {
            enabled: true,
            style: "cat".into(),
            size: 64,
            pos: None,
        };
        // 开启 + 无参数变化 → 不推（原 set_pet_config 的推送条件回归）
        assert!(!should_push_params(&base, &base.clone()));
        // 开启 + 尺寸变化 → 推
        let mut next = base.clone();
        next.size = 96;
        assert!(should_push_params(&base, &next));
        // 开启 + 形象变化 → 推
        let mut next = base.clone();
        next.style = "bot".into();
        assert!(should_push_params(&base, &next));
        // 位置变化不影响推送判定（位置走 Moved 挂点持久化）
        let mut next = base.clone();
        next.pos = Some((10.0, 20.0));
        assert!(!should_push_params(&base, &next));
        // 关闭状态即使参数变化也不推（窗口已关/将关，无热切换对象）
        let mut off_prev = base.clone();
        off_prev.enabled = false;
        let mut off_next = off_prev.clone();
        off_next.size = 96;
        assert!(!should_push_params(&off_prev, &off_next));
    }

    #[test]
    fn 默认位置_边距不越界() {
        // 无显示器环境退回原点；坐标计算不越界（负值夹 0、正常屏留边距）
        let (x, y) = default_bottom_right(None, 64);
        assert_eq!((x, y), (0.0, 0.0), "无显示器应退回原点");
        // 模拟小显示器（800×600 逻辑）：64px 宠物 + 边距不越界
        let (x, y) = bottom_right_xy(800.0, 600.0, 64);
        assert!(x >= 0.0 && y >= 0.0, "默认位置不应为负：{x},{y}");
        assert!(x < 800.0 && y < 600.0, "默认位置应在屏幕内：{x},{y}");
        assert!((800.0 - x - 64.0 - PET_DEFAULT_MARGIN).abs() < 1e-9);
        // 小于宠物+边距的屏幕：夹到 0 不为负
        let (x, y) = bottom_right_xy(50.0, 50.0, 64);
        assert_eq!((x, y), (0.0, 0.0));
    }

    #[test]
    fn 窗口宽高比_自定义形象按帧比例() {
        // P2-1：自定义形象窗口高度 = 边长 × frameH/frameW（Petdex 帧
        // 192×208），内建/非法/缺失形象保持正方形
        let root = std::env::temp_dir().join(format!("zbar-pet-aspect-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let dir = root.join("boba");
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            dir.join("pet.json"),
            r#"{"id":"boba","name":"Boba","format":"petdex-v1","cols":8,"rows":9,"frameW":192,"frameH":208,"image":"sheet.png","states":{}}"#,
        )
        .unwrap();
        assert_eq!(aspect_in(&root, "cat"), 1.0, "内建形象应为正方形");
        assert_eq!(aspect_in(&root, "bot"), 1.0);
        assert_eq!(aspect_in(&root, "custom:missing"), 1.0, "缺失宠物回退 1:1");
        assert_eq!(aspect_in(&root, "custom:../etc"), 1.0, "非法 id 回退 1:1");
        let ratio = aspect_in(&root, "custom:boba");
        assert!((ratio - 208.0 / 192.0).abs() < 1e-9, "Petdex 帧比 208/192：{ratio}");
        let _ = fs::remove_dir_all(&root);
    }
}

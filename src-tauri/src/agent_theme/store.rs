//! 主题数据落盘：目录布局、params.json、state.json、variables.css 重渲。
//!
//! 目录布局（~/.zbar/agent-themes/）：
//!   zcode/params.json     主题参数（ThemeParams，camelCase）
//!   zcode/state.json      安装状态与版本指纹缓存
//!   zcode/variables.css   由参数渲染出的 CSS 变量（注入物外链引用）
//!   zcode/theme.css       主题样式（版本化落盘：低于内置版本自动升级）
//!   zcode/effects.js      壁纸运行时脚本（版本化落盘，同上）
//!   zcode/usage.js        对话页用量统计条脚本（版本化落盘，同上）
//!   zcode/usage-data.js   用量数据（usage_feed 后台任务周期导出，非模板）
//!   zcode/wallpapers/     壁纸视频库（卸载还原时保留）
//!   zcode/backup/         原 app.asar 备份（仅保留 meta.json 指向的最新一份）+ meta.json
//!   zbar-staging-zcode-<ts>/  安装过程中的 asar 解包临时目录
//!   zbar-pack-zcode-<ts>.asar 安装过程中的重打包临时文件

use crate::agent_theme::inject;
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

// ============================================================
// 目录与文件名
// ============================================================

/// 主题根目录：~/.zbar/agent-themes
pub fn themes_dir() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("agent-themes"))
}

/// 单个应用的主题目录：~/.zbar/agent-themes/<app_id>
pub fn app_dir(app_id: &str) -> Result<PathBuf, String> {
    Ok(themes_dir()?.join(app_id))
}

/// ZCode 的主题目录（当前唯一支持的目标应用，语义便捷入口；
/// 供后续扩展的直查场景使用，业务主流程走 app_dir(app_id)）
#[allow(dead_code)]
pub fn zcode_dir() -> Result<PathBuf, String> {
    app_dir("zcode")
}

/// 壁纸目录：<app_dir>/wallpapers
pub fn wallpapers_dir(app_id: &str) -> Result<PathBuf, String> {
    Ok(app_dir(app_id)?.join(WALLPAPERS_DIR))
}

/// 备份目录：<app_dir>/backup
pub fn backup_dir(app_id: &str) -> Result<PathBuf, String> {
    Ok(app_dir(app_id)?.join(BACKUP_DIR))
}

pub const PARAMS_FILE: &str = "params.json";
pub const STATE_FILE: &str = "state.json";
pub const VARIABLES_CSS: &str = "variables.css";
pub const THEME_CSS: &str = "theme.css";
pub const EFFECTS_JS: &str = "effects.js";
/// 对话页用量统计条脚本（版本化落盘模板，inject::USAGE_JS）
pub const USAGE_JS: &str = "usage.js";
/// 用量数据文件（usage_feed 后台任务周期导出，非模板不做版本化；
/// usage.js 按自身 src 推导同目录地址加载）
pub const USAGE_DATA_FILE: &str = "usage-data.js";
pub const WALLPAPERS_DIR: &str = "wallpapers";
pub const BACKUP_DIR: &str = "backup";
pub const BACKUP_META_FILE: &str = "meta.json";
/// 默认壁纸文件名（由打包资源 wallpapers/ 落盘而来）
pub const DEFAULT_WALLPAPER_FILE: &str = "default.mp4";

// ============================================================
// 主题参数 ThemeParams
// ============================================================

/// 默认值常量：各参数的出厂默认（见 ThemeParams::default）
/// V3 起默认观感为"壁纸原样透出"：亮度/饱和度拉满、无模糊遮罩；
/// V5 主题分层后对话区与侧栏两个滑块各管各的容器、互不牵连，
/// 面板与侧栏均可安心默认全透明（其余区域由全局氛围底 base_alpha
/// 的默认值兜底）；
/// V6 新增右栏独立滑块，同样默认全透明
pub const DEFAULT_WP_BRIGHTNESS: f64 = 1.1;
pub const DEFAULT_WP_SATURATE: f64 = 1.4;
pub const DEFAULT_WP_BLUR: f64 = 0.0;
pub const DEFAULT_MASK_STRENGTH: f64 = 0.0;
pub const DEFAULT_PANEL_OPACITY: f64 = 0.0;
pub const DEFAULT_SIDEBAR_OPACITY: f64 = 0.0;
pub const DEFAULT_SIDEBAR_RIGHT_OPACITY: f64 = 0.0;
pub const DEFAULT_PLAYBACK_RATE: f64 = 1.0;
/// 全局氛围底不透明度默认值（用户可调参数 base_alpha，0~1）：
/// theme.css V5 起所有全局底色 token（:root 与 .dark 的
/// --color-background / -alt / --color-panel / --color-sidebar 共 8 条）
/// 统一由 --zbar-base-alpha 驱动，让顶栏、右侧面板、内容卡片等区域
/// 隐约透出壁纸的氛围底，同时与对话区/侧栏两个滑块彻底解绑——
/// 拖任一滑块只影响自己的主容器，不再牵连其余区域。
/// V10 起由固定常量（原 BASE_ALPHA）升级为用户可调参数：
/// variables.css 按参数渲染真值（热重载即时生效），theme.css 模板内
/// 的 0.25 兜底仅作旧 variables.css 的容错。
pub const DEFAULT_BASE_ALPHA: f64 = 0.25;
/// 文字描边强度默认值（用户可调参数 text_shadow，0~1，0=关闭）：
/// theme.css V10 起三区域主容器（侧栏 / 对话区 / 右栏）的前景文字
/// 按深浅主题补黑色/白色 text-shadow，强度消费 --zbar-text-shadow
/// （variables.css 渲染真值并随热重载即时生效），默认关闭不改变
/// 既有观感
pub const DEFAULT_TEXT_SHADOW: f64 = 0.0;
/// 对话内用量统计条字号默认值（用户可调参数 usage_font_size，9~16 整数 px）：
/// 取 usage.js V3 模板实际写死的 10px（font 简写 `400 10px/1.5`），
/// 升级 V4 后默认观感与旧版一致（V4 起消费 --zbar-usage-font-size，
/// variables.css 渲染真值并随热重载即时生效，模板内仅保留兜底）
pub const DEFAULT_USAGE_FONT_SIZE: f64 = 10.0;
/// 对话内用量统计条文字不透明度默认值（用户可调参数 usage_opacity，
/// 0.25~1）：取 usage.js V3 模板正常态实际写死的 opacity .55；等待态
/// 原写死 .35 的低透明观感由 V4 模板按正常态乘 0.636 系数保持
/// （0.55 × 0.636 ≈ 0.35）
pub const DEFAULT_USAGE_OPACITY: f64 = 0.55;
/// 会话级实时统计条默认开关（用户可调参数 usage_session_bar）：
/// usage.js V5 起在对话输入框上方固定悬浮当前会话累计条（Σ ↑↓⟲×，
/// 流式生成时动态跳动），默认开启；开关经 variables.css 的
/// --zbar-usage-session-bar（1/0）随热重载即时生效
pub const DEFAULT_USAGE_SESSION_BAR: bool = true;

/// 参数范围常量：(最小, 最大)
pub const WP_BRIGHTNESS_RANGE: (f64, f64) = (0.4, 1.1);
pub const WP_SATURATE_RANGE: (f64, f64) = (0.4, 1.4);
pub const WP_BLUR_RANGE: (f64, f64) = (0.0, 20.0);
pub const MASK_STRENGTH_RANGE: (f64, f64) = (0.0, 0.9);
pub const PANEL_OPACITY_RANGE: (f64, f64) = (0.0, 1.0);
pub const SIDEBAR_OPACITY_RANGE: (f64, f64) = (0.0, 1.0);
pub const SIDEBAR_RIGHT_OPACITY_RANGE: (f64, f64) = (0.0, 1.0);
pub const PLAYBACK_RATE_RANGE: (f64, f64) = (0.5, 2.0);
pub const BASE_ALPHA_RANGE: (f64, f64) = (0.0, 1.0);
pub const TEXT_SHADOW_RANGE: (f64, f64) = (0.0, 1.0);
pub const USAGE_FONT_SIZE_RANGE: (f64, f64) = (9.0, 16.0);
pub const USAGE_OPACITY_RANGE: (f64, f64) = (0.25, 1.0);

/// 动态壁纸主题参数（前端皮肤页的滑杆/表单数据）。
/// serde camelCase：与前端契约字段（wpBrightness / wallpaperFile 等）一一对应。
/// `#[serde(default)]`：旧版 params.json 缺字段时按默认值补齐。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ThemeParams {
    /// 壁纸亮度（0.4~1.1）
    pub wp_brightness: f64,
    /// 壁纸饱和度（0.4~1.4）
    pub wp_saturate: f64,
    /// 壁纸模糊半径 px（0~20）
    pub wp_blur: f64,
    /// 壁纸之上遮罩强度（0~0.9）
    pub mask_strength: f64,
    /// 对话区不透明度（0~1）
    pub panel_opacity: f64,
    /// 侧栏不透明度（0~1）
    pub sidebar_opacity: f64,
    /// 右栏不透明度（0~1）：data-pane-id 面板中除对话区四面板组
    /// （常驻主面板 workspace-main，V9 运行时实测：其为常驻 UI 中
    /// 唯一带面板属性的面板；及多面板视图才挂载的外层对话列
    /// conversation-column 与其内部 conversation / terminal 子面板）
    /// 外的全部面板（V7 修正：V6 误写为 data-panel-id），外加右栏
    /// "打开标签页"空态选择面板（无面板属性，按其容器类定位，
    /// V9 归属修正），V6 起与对话区/侧栏滑块彻底解绑（theme.css V9）
    pub sidebar_right_opacity: f64,
    /// 视频播放速率（0.5~2.0）
    pub playback_rate: f64,
    /// 全局氛围底不透明度（0~1，默认 0.25）：theme.css V5 起全部全局
    /// 底色 token 由 --zbar-base-alpha 驱动（语义详见
    /// DEFAULT_BASE_ALPHA 注释），V10 起由固定常量升级为本可调参数
    pub base_alpha: f64,
    /// 文字描边强度（0~1，0=关闭）：theme.css V10 起三区域主容器
    /// （侧栏 / 对话区 / 右栏）的前景文字按深浅主题补黑色/白色
    /// text-shadow，强度消费 --zbar-text-shadow，壁纸过亮/过暗时
    /// 把文字从背景里托出来，热重载即时生效
    pub text_shadow: f64,
    /// 对话内用量统计条字号（9~16 整数 px，默认 10 = usage.js V3 模板
    /// 写死值）：usage.js V4 起消费 --zbar-usage-font-size（variables.css
    /// 渲染真值，热重载即时生效），模板内仅保留同值兜底
    pub usage_font_size: f64,
    /// 对话内用量统计条文字不透明度（0.25~1，默认 0.55 = usage.js V3
    /// 模板正常态写死值）：usage.js V4 起消费 --zbar-usage-opacity；
    /// 等待态低透明观感由模板按本参数乘 0.636 系数保持（默认 ≈0.35）
    pub usage_opacity: f64,
    /// 会话级实时统计条开关（默认 true）：usage.js V5 起在 ZCode 对话
    /// 输入框上方固定悬浮当前会话累计条（Σ ↑非缓存输入 ↓输出 ⟲缓存读
    /// ×请求数，流式生成时动态跳动显示实时速度与估算增量），字号/不
    /// 透明度复用 usage_font_size / usage_opacity；开关经 variables.css
    /// 的 --zbar-usage-session-bar（1/0）热重载即时生效（usage.js 侧
    /// 变量缺失视为开启，兼容旧 variables.css）
    pub usage_session_bar: bool,
    /// 当前壁纸指向。语义（V3 起扩展）：
    /// - 绝对路径（以 / 或 Windows 盘符开头）→ 直接引用该文件
    /// - 相对文件名 → wallpapers/ 目录下的文件（如 "default.mp4"）
    pub wallpaper_file: Option<String>,
    /// 用户壁纸目录（wallpaper library 的扫描来源，绝对路径）；
    /// None = 未设置（仅扫描内置 wallpapers/ 目录）
    pub wallpaper_dir: Option<String>,
}

impl Default for ThemeParams {
    fn default() -> Self {
        Self {
            wp_brightness: DEFAULT_WP_BRIGHTNESS,
            wp_saturate: DEFAULT_WP_SATURATE,
            wp_blur: DEFAULT_WP_BLUR,
            mask_strength: DEFAULT_MASK_STRENGTH,
            panel_opacity: DEFAULT_PANEL_OPACITY,
            sidebar_opacity: DEFAULT_SIDEBAR_OPACITY,
            sidebar_right_opacity: DEFAULT_SIDEBAR_RIGHT_OPACITY,
            playback_rate: DEFAULT_PLAYBACK_RATE,
            base_alpha: DEFAULT_BASE_ALPHA,
            text_shadow: DEFAULT_TEXT_SHADOW,
            usage_font_size: DEFAULT_USAGE_FONT_SIZE,
            usage_opacity: DEFAULT_USAGE_OPACITY,
            usage_session_bar: DEFAULT_USAGE_SESSION_BAR,
            wallpaper_file: Some(DEFAULT_WALLPAPER_FILE.to_string()),
            wallpaper_dir: None,
        }
    }
}

fn clamp(v: f64, (min, max): (f64, f64)) -> f64 {
    v.clamp(min, max)
}

impl ThemeParams {
    /// 把越界参数收敛到合法范围（保存前的防御，脏数据不落盘）。
    pub fn clamped(mut self) -> Self {
        self.wp_brightness = clamp(self.wp_brightness, WP_BRIGHTNESS_RANGE);
        self.wp_saturate = clamp(self.wp_saturate, WP_SATURATE_RANGE);
        self.wp_blur = clamp(self.wp_blur, WP_BLUR_RANGE);
        self.mask_strength = clamp(self.mask_strength, MASK_STRENGTH_RANGE);
        self.panel_opacity = clamp(self.panel_opacity, PANEL_OPACITY_RANGE);
        self.sidebar_opacity = clamp(self.sidebar_opacity, SIDEBAR_OPACITY_RANGE);
        self.sidebar_right_opacity = clamp(self.sidebar_right_opacity, SIDEBAR_RIGHT_OPACITY_RANGE);
        self.playback_rate = clamp(self.playback_rate, PLAYBACK_RATE_RANGE);
        self.base_alpha = clamp(self.base_alpha, BASE_ALPHA_RANGE);
        self.text_shadow = clamp(self.text_shadow, TEXT_SHADOW_RANGE);
        self.usage_font_size = clamp(self.usage_font_size, USAGE_FONT_SIZE_RANGE);
        self.usage_opacity = clamp(self.usage_opacity, USAGE_OPACITY_RANGE);
        if !self.wallpaper_file.as_deref().is_some_and(|s| !s.trim().is_empty()) {
            self.wallpaper_file = Some(DEFAULT_WALLPAPER_FILE.to_string());
        }
        self
    }

    /// 当前壁纸指向（始终有值，缺省回 default.mp4）。
    /// 返回值可能是相对文件名（wallpapers/ 下）或绝对路径，语义见字段注释
    pub fn wallpaper_name(&self) -> &str {
        self.wallpaper_file
            .as_deref()
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(DEFAULT_WALLPAPER_FILE)
    }
}

// ============================================================
// 壁纸文件类型与路径解析
// ============================================================

/// 受支持的视频扩展名（小写，不带点）
pub const WALLPAPER_VIDEO_EXTS: [&str; 3] = ["mp4", "webm", "mov"];
/// 受支持的图片扩展名（小写，不带点）
pub const WALLPAPER_IMAGE_EXTS: [&str; 4] = ["jpg", "jpeg", "png", "webp"];

/// 按文件扩展名判定壁纸类型：返回 "video" / "image"，
/// 不受支持的扩展名返回 None（导入校验与目录扫描共用）
pub fn wallpaper_kind_of(file_name: &str) -> Option<&'static str> {
    let ext = Path::new(file_name)
        .extension()?
        .to_str()?
        .to_ascii_lowercase();
    if WALLPAPER_VIDEO_EXTS.contains(&ext.as_str()) {
        Some("video")
    } else if WALLPAPER_IMAGE_EXTS.contains(&ext.as_str()) {
        Some("image")
    } else {
        None
    }
}

/// wallpaper_file 指向是否为绝对路径（Unix / 开头或 Windows 盘符开头，
/// 由 Path::is_absolute 覆盖两种平台形态）
fn is_absolute_ref(name: &str) -> bool {
    Path::new(name).is_absolute()
}

/// 把 params 中的壁纸指向解析为实际文件路径：
/// 绝对路径直接引用；相对文件名拼 wallpapers/ 目录（default.mp4 兜底语义
/// 由 wallpaper_name 保证）。返回 (路径, 是否绝对引用)
fn resolve_wallpaper_path(dir: &Path, name: &str) -> (PathBuf, bool) {
    if is_absolute_ref(name) {
        (PathBuf::from(name), true)
    } else {
        (dir.join(WALLPAPERS_DIR).join(name), false)
    }
}

/// 读取参数；文件不存在或内容损坏时静默返回默认值（皮肤页首开无 params.json）。
pub fn load_params(app_id: &str) -> ThemeParams {
    let Ok(path) = params_path(app_id) else {
        return ThemeParams::default();
    };
    read_params_file(&path).unwrap_or_default()
}

/// 从指定路径读参数（供单元测试复用）。失败返回 None。
/// 读取时归一化 wallpaperFile 的 Windows verbatim 前缀：历史版本曾把
/// canonicalize 产生的 `\\?\C:\…` 原样落盘，渲染成 Chromium 无法加载的
/// 坏 URL；内存中剥前缀自愈，下一次落盘自然写回干净路径。
pub(crate) fn read_params_file(path: &Path) -> Option<ThemeParams> {
    let text = fs::read_to_string(path).ok()?;
    let mut params: ThemeParams = serde_json::from_str(&text).ok()?;
    if let Some(wp) = &mut params.wallpaper_file {
        *wp = inject::strip_verbatim_prefix(PathBuf::from(wp.as_str()))
            .to_string_lossy()
            .to_string();
    }
    Some(params)
}

/// 保存参数：clamp 后原子性写入（先写临时文件再改名，避免半截 JSON）。
pub fn save_params(app_id: &str, params: &ThemeParams) -> Result<(), String> {
    let dir = app_dir(app_id)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
    write_params_file(&dir.join(PARAMS_FILE), &params.clone().clamped())
}

/// 写参数到指定路径（供单元测试复用）。
pub(crate) fn write_params_file(path: &Path, params: &ThemeParams) -> Result<(), String> {
    let json = serde_json::to_string_pretty(params)
        .map_err(|e| format!("序列化主题参数失败: {e}"))?;
    let tmp = path.with_extension("json.tmp");
    fs::write(&tmp, json).map_err(|e| format!("写入主题参数失败: {e}"))?;
    fs::rename(&tmp, path).map_err(|e| format!("保存主题参数失败: {e}"))
}

fn params_path(app_id: &str) -> Result<PathBuf, String> {
    Ok(app_dir(app_id)?.join(PARAMS_FILE))
}

// ============================================================
// 安装状态 state.json
// ============================================================

pub const STATUS_INSTALLED: &str = "installed";
pub const STATUS_INSTALLING: &str = "installing";
/// installing 状态的陈旧判定阈值（秒）：超过视为上次异常中断的残留，允许重新安装
pub const INSTALLING_STALE_SECS: i64 = 600;

/// 安装状态（含注入标记检测结果缓存与版本指纹）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredState {
    /// "installed" / "installing"；None 表示未安装
    pub status: Option<String>,
    /// 安装时的目标应用版本（指纹比对：应用升级后提示重装）
    pub zcode_version: Option<String>,
    /// 安装时的 asar 体积（注入标记检测的缓存 key：体积未变则信任缓存）
    pub asar_size: Option<u64>,
    /// 安装时的 asar 最后修改时间（Unix 秒）：与体积共同构成缓存 key，
    /// 覆盖"体积相同但内容被触碰"的场景；None（旧版 state.json 或
    /// mtime 读取失败）时缓存判定退化为仅体积匹配
    pub asar_mtime: Option<i64>,
    /// 安装完成时间（RFC3339）
    pub injected_at: Option<String>,
    /// asar 内注入标记检测结果缓存
    pub injected_marker: bool,
    /// 进入 installing 状态的 Unix 时间戳（秒）
    pub installing_since: Option<i64>,
}

impl StoredState {
    /// 是否处于已安装状态（status == installed）
    #[allow(dead_code)] // 单元测试与后续状态查询使用
    pub fn is_installed(&self) -> bool {
        self.status.as_deref() == Some(STATUS_INSTALLED)
    }

    /// 是否处于"近期内的 installing"（用于拦截并发安装；超时视为残留放行）
    pub fn is_installing_recent(&self) -> bool {
        if self.status.as_deref() != Some(STATUS_INSTALLING) {
            return false;
        }
        match self.installing_since {
            Some(s) => chrono::Utc::now().timestamp() - s < INSTALLING_STALE_SECS,
            None => false,
        }
    }
}

/// 读取状态；文件不存在/损坏返回默认（未安装态，不报错）。
pub fn load_state(app_id: &str) -> StoredState {
    let Ok(path) = app_dir(app_id).map(|d| d.join(STATE_FILE)) else {
        return StoredState::default();
    };
    read_state_file(&path).unwrap_or_default()
}

pub(crate) fn read_state_file(path: &Path) -> Option<StoredState> {
    let text = fs::read_to_string(path).ok()?;
    serde_json::from_str(&text).ok()
}

/// 写状态文件（目录不存在自动创建）。
pub fn save_state(app_id: &str, state: &StoredState) -> Result<(), String> {
    let dir = app_dir(app_id)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
    let json =
        serde_json::to_string_pretty(state).map_err(|e| format!("序列化安装状态失败: {e}"))?;
    fs::write(dir.join(STATE_FILE), json).map_err(|e| format!("写入安装状态失败: {e}"))
}

/// 状态复位为未安装（卸载完成 / 安装失败回滚后调用）。
pub fn reset_state(app_id: &str) {
    let _ = save_state(app_id, &StoredState::default());
}

// ============================================================
// 备份 meta
// ============================================================

/// 备份文件名：app.asar.v<版本>.<原字节数>.bak（版本中的非常规字符替换为 -）。
/// mod.rs 的备份写入与本文件的 meta 驱动选择共用同一构造，避免两处格式漂移。
pub(crate) fn backup_file_name(version: Option<&str>, size: u64) -> String {
    let v: String = version
        .unwrap_or("unknown")
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '.' { c } else { '-' })
        .collect();
    format!("app.asar.v{v}.{size}.bak")
}

/// 备份元信息（backup/meta.json），用于还原前完整性校验。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BackupMeta {
    /// 备份对应原 asar 的字节数
    pub asar_size: u64,
    /// 备份时的应用版本
    pub zcode_version: Option<String>,
    /// 备份时间（RFC3339）
    pub created_at: String,
}

/// 写备份元信息到显式目录（备份守卫步骤与单元测试复用，不依赖真实 ~/.zbar）。
pub(crate) fn write_backup_meta_in(
    dir: &Path,
    asar_size: u64,
    zcode_version: Option<String>,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let meta = BackupMeta {
        asar_size,
        zcode_version,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    let json =
        serde_json::to_string_pretty(&meta).map_err(|e| format!("序列化备份信息失败：{e}"))?;
    fs::write(dir.join(BACKUP_META_FILE), json).map_err(|e| format!("写入备份信息失败：{e}"))
}

/// 读备份元信息（缺失返回 None）。
pub fn load_backup_meta(app_id: &str) -> Option<BackupMeta> {
    load_backup_meta_in(&backup_dir(app_id).ok()?)
}

/// 读显式目录下的备份元信息（缺失返回 None，单元测试复用）。
pub(crate) fn load_backup_meta_in(dir: &Path) -> Option<BackupMeta> {
    let text = fs::read_to_string(dir.join(BACKUP_META_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}

/// 最新备份文件路径（meta 驱动，mtime 兜底）。
///
/// 【真机事故复盘，勿回退】用户经历 ZCode 3.9.2 → 3.10.0 升级后，backup
/// 目录同时存在两个版本的 .bak（v3.9.2 与 v3.10.0）。3.10.0 重装时同版本
/// 备份已存在、拷贝被跳过（幂等，合理），旧 v3.9.2.bak 的 mtime 反而更新
/// → 旧实现按 mtime 取最新时选中了 v3.9.2 文件，配 v3.10.0 的 meta.json
/// → 完整性校验（记录 307008658 字节 / 实际 297625072 字节）拒绝还原。
/// 结论：mtime 排序在多版本备份共存时不可靠。
///
/// meta.json 是"最后一次成功备份"的权威记录（version + asarSize）→
/// 直接按备份文件名精确查找：存在即返回；meta 缺失/损坏/指向的文件
/// 不存在时，降级为按 mtime 取最新 .bak 的旧逻辑兜底。
pub fn latest_backup(app_id: &str) -> Option<PathBuf> {
    let dir = backup_dir(app_id).ok()?;
    latest_backup_in(&dir)
}

/// latest_backup 的目录显式版（单元测试复用，不依赖真实 ~/.zbar）。
pub(crate) fn latest_backup_in(dir: &Path) -> Option<PathBuf> {
    // 主路径：meta 驱动精确查找（最后成功备份的版本 + 体积 → 精确文件名）
    if let Some(meta) = load_backup_meta_in(dir) {
        let candidate = dir.join(backup_file_name(meta.zcode_version.as_deref(), meta.asar_size));
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    // 兜底：meta 缺失/损坏/指向文件不存在 → 按 mtime 取最新
    latest_by_mtime_in(dir)
}

/// 按 mtime 取最新 .bak（历史兜底逻辑；多版本备份共存时不可靠，见
/// latest_backup 的复盘注释，仅作 meta 不可用时的降级路径）。
fn latest_by_mtime_in(dir: &Path) -> Option<PathBuf> {
    let mut best: Option<(std::time::SystemTime, PathBuf)> = None;
    for entry in fs::read_dir(dir).ok()?.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if !name.ends_with(".bak") {
            continue;
        }
        let Ok(modified) = entry.metadata().and_then(|m| m.modified()) else {
            continue;
        };
        let newer = best.as_ref().map_or(true, |(t, _)| modified > *t);
        if newer {
            best = Some((modified, entry.path()));
        }
    }
    best.map(|(_, p)| p)
}

/// 删除备份目录内除 `keep` 之外的全部旧版本 `app.asar.v*.bak`。
///
/// 【背景】还原永远只应使用 meta 指向的最新备份，旧版本备份纯占空间
/// （每个约 300MB）；且多版本备份共存曾让按 mtime 的旧选择逻辑选错文件
/// （见 latest_backup 复盘注释），清理旧备份可从源头杜绝复发。
/// 仅匹配 `app.asar.v` 前缀 + `.bak` 后缀的常规文件，meta.json 等其他
/// 内容一律不动。清理失败返回 Err 由调用方记日志，不阻塞备份主流程
/// （多留一份旧备份不影响 meta 驱动的正确选择）。
pub(crate) fn remove_stale_backups_in(dir: &Path, keep: &Path) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|e| format!("读取备份目录失败：{e}"))?.flatten() {
        let path = entry.path();
        if path == keep || !path.is_file() {
            continue;
        }
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with("app.asar.v") && name.ends_with(".bak") {
            fs::remove_file(&path)
                .map_err(|e| format!("删除旧版本备份失败（{}）: {e}", path.display()))?;
        }
    }
    Ok(())
}

// ============================================================
// variables.css 重渲与主题资产落盘
// ============================================================

/// 指定目录内的 variables.css 重渲（ensure_theme_assets / apply_wallpaper
/// 内部调用与单元测试共用，不依赖真实 ~/.zbar）。
/// wallpaperFile 语义（V3）：绝对路径直接 file_url 引用；相对文件名拼
/// wallpapers/ 目录；缺失/为空时回落 default.mp4。文件不存在时由
/// effects.js 对加载失败静默移除、退回原生观感（指向信息不丢失）。
/// 幂等优化：内容无变化时跳过写盘——皮肤页状态轮询会反复触发本函数，
/// 而 effects.js 每秒热重载重读 variables.css，无谓的重写只添噪。
pub(crate) fn refresh_variables_css_in(dir: &Path) -> Result<(), String> {
    let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap_or_default();
    let (wp_path, _) = resolve_wallpaper_path(dir, params.wallpaper_name());
    let url = inject::file_url(&wp_path);
    let css = inject::render_variables_css(&params, &url);
    let path = dir.join(VARIABLES_CSS);
    if fs::read_to_string(&path).is_ok_and(|old| old == css) {
        return Ok(());
    }    fs::write(&path, css).map_err(|e| format!("写入 variables.css 失败: {e}"))
}

/// 壁纸导入后的指向切换（set_agent_wallpaper 拷贝成功后调用）：
/// 把新指向写入 params.json 的 wallpaperFile，再按新参数重渲 variables.css，
/// CSS 变量即指向新壁纸。换壁纸的指向切换由导入命令全权负责完成。
/// `file_ref` 为相对文件名（wallpapers/ 下）或绝对路径，语义同字段注释。
pub fn apply_wallpaper(app_id: &str, file_ref: &str) -> Result<(), String> {
    let dir = app_dir(app_id)?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
    apply_wallpaper_in(&dir, file_ref)
}

/// 指定目录内的"更新壁纸指向 + 重渲"核心（app_id 版与单元测试共用）。
pub(crate) fn apply_wallpaper_in(dir: &Path, file_ref: &str) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
    let path = dir.join(PARAMS_FILE);
    let mut params = read_params_file(&path).unwrap_or_default();
    params.wallpaper_file = Some(file_ref.to_string());
    // 与 save_params 同样先 clamp，越界脏数据不落盘
    write_params_file(&path, &params.clamped())?;
    refresh_variables_css_in(dir)
}

// ============================================================
// 壁纸库：目录扫描 / 选择校验 / 目录设置
// ============================================================

/// 扫描目录内受支持的壁纸文件，按文件名排序。
/// `recursive=true` 时全递归（用户壁纸目录），否则仅顶层（内置 wallpapers/
/// 目录为平铺结构）。隐藏文件与隐藏目录（点开头）一律过滤。
pub(crate) fn collect_wallpapers_in(dir: &Path, recursive: bool) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        // 过滤隐藏文件/目录（macOS 的 .DS_Store、点开头素材目录等）
        if name.starts_with('.') {
            continue;
        }
        let path = entry.path();
        if path.is_dir() {
            if recursive {
                out.extend(collect_wallpapers_in(&path, true));
            }
        } else if wallpaper_kind_of(&name).is_some() {
            out.push(path);
        }
    }
    sort_by_file_name(&mut out);
    out
}

/// 按文件名排序（大小写敏感字节序，跨平台稳定）
fn sort_by_file_name(paths: &mut [PathBuf]) {
    paths.sort_by(|a, b| a.file_name().cmp(&b.file_name()));
}

/// 聚合壁纸库全部文件（不含内置默认项，默认项由命令层 DTO 固定补首）。
/// 两个来源互斥，避免设置目录后旧导入内容混入列表：
/// - 已设置用户壁纸目录（存在可解析）：仅返回该目录扫描结果（全递归），
///   不再聚合内置 wallpapers/ 目录——目录语义即"清单 = 该目录全部内容"；
/// - 未设置：仅扫描内置 wallpapers/ 目录（平铺，兼容单文件拖入导入的
///   旧用法），并按文件名过滤 default.mp4 实体（它就是默认项本身，
///   不过滤会与 DTO 补首的默认项重复成两张卡片）。
pub(crate) fn list_wallpapers_in(dir: &Path, wp_dir: &Path) -> Vec<PathBuf> {
    if let Some(user_dir) = user_wallpaper_dir(dir) {
        return collect_wallpapers_in(&user_dir, true);
    }
    let mut files = collect_wallpapers_in(wp_dir, false)
        .into_iter()
        .filter(|p| !p.file_name().is_some_and(|n| n == DEFAULT_WALLPAPER_FILE))
        .collect::<Vec<_>>();
    sort_by_file_name(&mut files);
    files
}

/// 读取 params 中的用户壁纸目录并 canonicalize（未设置/不存在/空值返回 None）
fn user_wallpaper_dir(dir: &Path) -> Option<PathBuf> {
    let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap_or_default();
    let raw = params
        .wallpaper_dir
        .as_deref()?
        .trim()
        .to_string();
    if raw.is_empty() {
        return None;
    }
    Path::new(&raw)
        .canonicalize()
        .ok()
        .map(inject::strip_verbatim_prefix)
}

/// 壁纸库选择校验：路径必须位于 wallpapers/ 或用户壁纸目录内
/// （canonicalize 后做前缀比对，防任意路径注入把 variables.css 指向
/// 敏感文件），且必须是受支持类型的常规文件。
/// 返回 canonicalize 并剥离 Windows verbatim 前缀的绝对路径
/// （存入 params.wallpaper_file；verbatim 形态会被渲染成 Chromium
/// 无法加载的坏 URL）。
pub(crate) fn resolve_selectable_wallpaper_in(
    dir: &Path,
    wp_dir: &Path,
    raw: &str,
) -> Result<PathBuf, String> {
    let canon = inject::strip_verbatim_prefix(
        Path::new(raw)
            .canonicalize()
            .map_err(|_| format!("壁纸文件不存在：{raw}"))?,
    );
    if !canon.is_file() {
        return Err(format!("不是壁纸文件：{raw}"));
    }
    if wallpaper_kind_of(&canon.to_string_lossy()).is_none() {
        return Err(
            "仅支持 mp4 / webm / mov 视频与 jpg / jpeg / png / webp 图片".into(),
        );
    }
    // 白名单根：内置 wallpapers/ 目录 + 用户壁纸目录（同样剥前缀，
    // 保证与 canon 的前缀比对口径一致）
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(canon_wp) = wp_dir.canonicalize() {
        roots.push(inject::strip_verbatim_prefix(canon_wp));
    }
    if let Some(user_dir) = user_wallpaper_dir(dir) {
        roots.push(user_dir);
    }
    if roots.iter().any(|r| canon.starts_with(r)) {
        Ok(canon)
    } else {
        Err("壁纸路径必须位于壁纸目录内（wallpapers/ 或已设置的壁纸目录）".into())
    }
}

/// 壁纸库选择核心（select_agent_wallpaper 的目录显式版，单元测试共用）：
/// - `raw == "default"` → 指向回落 default.mp4
/// - 其余 → 越界/类型校验后把绝对路径写入 params.wallpaper_file 并重渲
pub(crate) fn select_wallpaper_in(dir: &Path, wp_dir: &Path, raw: &str) -> Result<(), String> {
    let raw = raw.trim();
    if raw == "default" {
        return apply_wallpaper_in(dir, DEFAULT_WALLPAPER_FILE);
    }
    let target = resolve_selectable_wallpaper_in(dir, wp_dir, raw)?;
    apply_wallpaper_in(dir, &target.to_string_lossy())
}

/// 设置/清除用户壁纸目录（set_agent_wallpaper_dir 的落盘实现；
/// None = 清除）。不影响 variables.css（目录本身不参与渲染）。
pub fn set_wallpaper_dir(app_id: &str, dir: Option<String>) -> Result<(), String> {
    let root = app_dir(app_id)?;
    fs::create_dir_all(&root).map_err(|e| format!("创建主题目录失败: {e}"))?;
    let path = root.join(PARAMS_FILE);
    let mut params = read_params_file(&path).unwrap_or_default();
    params.wallpaper_dir = dir;
    write_params_file(&path, &params.clamped())
}

// ============================================================
// 主题模板版本化升级
// ============================================================

/// 主题模板版本（已落盘 theme.css / effects.js 头部版本标记
/// "ZBAR-THEME-V" + 数字，两个文件各自独立比较）：
/// 低于对应内置版本时由 ensure_theme_assets 用内置模板覆盖升级；
/// 等于则视为用户实机调优过的版本，不动。
/// 升级只触碰这两个模板文件，用户的 params.json / wallpapers /
/// variables.css 不受影响——参数变化经 variables.css 每秒热重载即时
/// 生效；模板文件本身的升级经面板"重启 ZCode"冷启动完全重载生效，
/// 旧 asar 注入行（无 data 标记）无需重装主题。
/// theme.css V6（对话区作用区修正 + 右栏独立滑块：实测 #content 是
/// "对话列 + 右侧面板"的整体容器，V5 把对话区规则刷在 #content 上会
/// 牵连右侧面板；V6 删除 #content 元素规则，改按 react-resizable-panels
/// 的面板属性分层——对话列 conversation-column 消费
/// --zbar-panel-opacity，右栏其余面板用 :not 反选消费新增的
/// --zbar-sidebar-right-opacity（自动覆盖将来新增面板），与左栏
/// #sidebar（--zbar-sidebar-opacity）构成三区域独立分层）；
/// theme.css V7（选择器属性名修正：面板容器 DOM 实为 data-pane-id，
/// V6 误写 data-panel-id 致对话列与右栏选择器落空，V7 仅改属性名）；
/// theme.css V9（运行时实测选择器终版修正：常驻 UI 中唯一带面板属性
/// 的面板是 workspace-main，V8 枚举的对话列三面板组仅在多面板视图
/// 展开时才挂载；V7/V8 症状根因是右栏 :not 反选把 workspace-main
/// 捞走、对话区 conversation-column 选择器常驻无命中。V9 对话区
/// 作用区扩为四面板组（workspace-main + 对话列三面板组），右栏
/// :not 链同步排除四个面板值，无面板属性的右栏"打开标签页"空态
/// 选择面板（容器类 side-pane-open-tab-shell）并入右栏作用区）；
/// theme.css V10（文字可读性增强：--zbar-base-alpha 由固定常量改为
/// 用户可调参数 base_alpha、新增文字描边参数 text_shadow 消费
/// --zbar-text-shadow，文件末尾追加三区域主容器的前景文字描边规则——
/// 深色主题黑描边、浅色主题白描边，强度 0 时 alpha 为 0 不可见；
/// 既有 V9 选择器与规则一字未动，仅文件末尾追加）；
/// effects.js V3（图片壁纸支持）；
/// effects.js V4（theme.css 纳入每秒 cache-bust 热重载：theme.css 是
/// 静态 link 不随参数热更新，已装用户磁盘模板升级后运行中的 ZCode
/// 页面仍持旧样式——如 V10 新增的文字描边滑块拖动零反馈；与
/// variables.css 同款 href 时间戳强制重读后免重启换用新样式，按
/// data-zbar-theme 定位注入行，找不到时静默跳过）；
/// effects.js V5（撤销 V4 的 theme.css 每秒热重载 + poll 竞态防御：
/// 样式表 href 变更会经历"旧样式表卸载失效 → 异步加载解析 → 恢复"
/// 窗口，失效窗口内 theme.css 的三区域背景/文字描边规则整体失效，
/// 背景闪回 ZCode 原生底色，系统忙时窗口拉长到肉眼可见的周期性
/// 闪烁，故删除 reloadThemeLink；theme.css 模板升级改由面板"重启
/// ZCode"按钮冷启动完全重载闭环。同时 poll 调整为"先 snapshotOf
/// 取快照、后 reloadVarsLink 重读"，避免快照撞上本轮 variables.css
/// 重载窗口；并加空值防御——快照中任一 --zbar-* 变量读到空串视为
/// 失效窗口，本轮直接返回，防止误判参数变化把滤镜/遮罩/壁纸重置
/// 为默认值）
pub const THEME_CSS_VERSION: u32 = 11;
pub const EFFECTS_JS_VERSION: u32 = 5;
/// usage.js V1（对话页用量统计条首版：每轮对话下方渲染 ↑ 输入 ↓ 输出
/// ⟲ 缓存读 · 请求数 · 输出速度 · 首字延迟，数据源为 usage_feed 导出的
/// 同目录 usage-data.js；选择器常量集中在文件头部便于实机调整）；
/// usage.js V2（实机缺陷修复：实测 DOM data-turn-id 的值是 msg_ 前缀的
/// user_message_id 而非 turn_id，匹配键改用数据 v2 新增的 umid 字段；
/// 同一轮的多个虚拟列表单元节点只在 DOM 顺序最后一个节点渲染；数据 ts
/// 未变时跳过重建与重渲染。usage.js 为外链注入——模板升级落盘后，用户
/// 重启 ZCode 冷启动重载页面即生效新脚本，无需重装 asar）；
/// usage.js V3（修复 scheduleRender 异常死锁：rAF 回调改 try/finally，
/// scheduled 复位移入 finally；另加 15 秒低频兜底渲染自愈）；
/// usage.js V4（样式参数化：统计条字号/不透明度改由 variables.css 的
/// --zbar-usage-font-size / --zbar-usage-opacity 驱动（皮肤页新增
/// "用量统计条"滑块可调，随每秒热重载即时生效），模板内仅保留 V3
/// 写死值（10px / .55）兜底；等待态低透明由正常态乘 0.636 系数得出
/// （默认 0.55×0.636≈0.35，与 V3 写死观感一致）。渲染/匹配/死锁
/// 防护逻辑零改动）；
/// usage.js V5（行格式图标化 + 会话级实时统计条：每轮行 "N req" →
/// "× N"、"tok/s" → "t/s"、"首字" → "TTFT"，仅显示层数据与渲染管线
/// 零改动；新增 renderSessionBar 会话条——fixed 悬浮于对话输入框上方
/// （锚点 workspace-main，bottom 常量实机可调），显示当前会话真实累计
/// Σ ↑↓⟲×（按 sess 过滤 turns 聚合，含并入的子代理部分），轮进行中时
/// 追加 "⋯ t/s · ↓ ~估算" 动态段（textContent.length 差分 × 3.5 字符/
/// token 估算，滑动窗口求速，200ms 刷新），轮完成即切回数据库真实值；
/// 开关由新增参数 usage_session_bar 经 variables.css 的
/// --zbar-usage-session-bar（1/0）热重载生效，数据格式 v2 不变）；
/// usage.js V6（生成过程实时跳动，Claude Code CLI 式）：数据源追加
/// runs 数组（usage_feed 从 model_usage 聚合"已落库至少一步请求、
/// turn_usage 尚未写入"的进行中轮，v2 契约不变附加字段，旧脚本忽略
/// 未知字段平滑兼容）——每轮条渲染优先级改为完成数据 > 进行中 run
/// （runIndex 命中，真实聚合 + DOM 流式估算叠加 ↓ + 估算速度段 +
/// "…" 尾缀）> data-running 等待态，轮完成 2 秒内自然切最终真实值；
/// 会话条 Σ = 完成轮合计 + runs（sess 或 psess 命中）+ 流式估算，
/// 新会话首轮也即时显示。修复 V5 动态段不生效的两个根因：
/// renderSessionBar 在会话无完成轮时提前返回致定时器从未启动；动态
/// 检测只依赖 data-running="true" 实机不可靠——目标节点改由 runIndex
/// 数据驱动（findLiveNode），data-running 仅作数据未达头 2 秒兜底）
/// usage.js V7（请求次数图标 ⟳ → ×：原 ⟳ 与缓存读 ⟲ 仅箭头方向之差
/// 过于相似易混淆，改乘号 × 读作"共 N 次"；缓存读 ⟲ 与其余格式不变）；
/// usage.js V8（启动窗口实时渲染，修复实机缺陷"发消息后每轮条不显示、
/// 会话累计条不动，直到第一笔模型请求完成后才有内容"）：V6/V7 的实时
/// 渲染以数据驱动——live 每轮条与估算目标（findLiveNode）都依赖
/// runIndex（runs = model_usage 已完成请求的聚合），轮次开始到首笔请求
/// 完成之间（思考阶段可达几十秒）model_usage 无行 → runs 无该轮 → live
/// 条不渲染、估算无目标、会话条 sessionRunTotals 为空，启动窗口全空白
/// （data-running 兜底实机不可靠）。V8 活动轮判定改为 DOM 驱动：会话内
/// DOM 顺序最后一个 data-turn-id（umid）节点，其 umid 不在 index 即为
/// 活动轮——既不在 index 也不在 runIndex = 启动窗口活动轮（消息发出节
/// 点即在 DOM，无需任何数据库数据）：每轮条立即渲染动态段起始态
/// "⋯ X.X t/s · ↓ ~X …"（真实部分全 0 不渲染数字段，估算来自 DOM，
/// title 说明第一笔请求完成后显示真实用量），runIndex 命中后切 live 完
/// 整格式；估算目标统一为该活动轮节点（删除 runs 目标依赖与 data-running
/// 依赖，等待态渲染分支删除）；会话条放弃渲染条件追加"无活动轮"，估
/// 算输出改入动态段 ↓ ~X（不再叠加进 Σ ↓ 真实数字）；轮完成切换与估
/// 算器重置路径不变）
/// usage.js V9（子代理消耗实时化，实机反馈：子代理运行时主对话的每轮
/// 条和会话累计条都不随子代理消耗动态更新，子代理详情面板也不显示自己
/// 的统计）：实机核实子代理详情面板与主对话同 document（无 iframe），
/// 面板有自己的 [data-session-id] 容器（值为子代理会话 id）与
/// [data-turn-id] 节点（值为子代理会话自己的 umid，与数据 runs 子代理
/// 行一致），面板默认关闭、点开才挂载。V8 三个缺陷：renderAll 扫描限定
/// workspace-main 锚点（面板在锚点外扫不到）；主轮 runs 行只聚合主会话
/// 自身 model_usage（子代理跑时主轮条/会话条不动）；主轮未完成时其子代
/// 理完成轮不进 turns 也不进 runs（消耗在界面丢失，会话 Σ 缺数）。
/// V9 渲染端：renderAll 与活动轮判定改 document 级扫描（35 节点量级，
/// 锚点仅保留给会话条定位），子代理面板每轮条随扫描按子代理行 umid 命
/// 中渲染 live 条；活动轮判定改多容器（findLiveNodes 遍历所有
/// [data-session-id] 容器，每容器 DOM 最后 umid 不在完成索引的节点为
/// 该会话活动轮，返回 Map：sess → 节点，并行多子代理各有活动轮）；估
/// 算器多目标（dyn.targets：sess → {node, 基准长度, 窗口样本}，单一
/// 200ms 定时器统一采样驱动）；主轮 live 行数字合计数据侧并入的 sub
/// （子代理消耗实时反映在主轮条），title 分解"含子代理 n 轮"；会话条
/// Σ 跳过 m:1 子代理行（其值已并入主轮行 sub，随主轮行一并计入），
/// 修复原 psess 裸命中的双计/丢失；关闭会话条不再连带停估算。
/// V9 数据端（usage_feed，数据格式仍 v2 附加字段向后兼容）：主会话
/// runs 行新增 sub 聚合 = psess 指向该会话的子代理 runs 行（并行多子
/// 代理全并）+ 游离子代理完成轮（turns/merge 阶段按时间窗口分流：父
/// 会话无覆盖子轮开始时刻的完成主轮 = 所属主轮未落库才输出，防与
/// turns 整轮并入双计）；子代理 runs 行在父会话存在主轮行时打 m:1）；
/// usage.js V10（统计条显示稳定性，用户反馈：统计条图标段数一会多一
/// 会少、左右宽度持续变化，观感差——原三态各一种结构且"有值才显示、
/// 无值省略"）：每轮条三态（启动窗口/live/完成）统一为同一固定结构
/// "↑ in ↓ out ⟲ cr · × req · speed t/s · TTFT ttft"，任何状态都渲
/// 染全部字段位、只更新数值；数字等宽补位（token 5 字符 / req 3 字符
/// / 速度与 TTFT 各 4 字符，等宽字体 + tabular-nums 下整行宽度恒定；
/// dur 缺失速度占位、进行中/缺失 TTFT 显示 "–"）；↓ 前固定 1 字符估
/// 算前缀位（进行中 "~"、完成空格）；启动窗口极简行与行尾 "…" 进行
/// 中标记删除（合并进统一格式函数 barLineOf）；会话条动态段
/// "⋯ t/s · ↓ ~est" 两段永远显示（idle 时 0.0 / 0），Σ 数字段同步补
/// 位；顺带修复已完成子代理面板的残留占位（枯萎判定：活动轮目标文本
/// 连续 STALE_MS=90 秒无增长且 umid 始终不在 index/runIndex → 移除
/// 残留占位行并从活动轮目标中移除，文本再变化时重新评估）。数据解析、
/// runs/turns 索引、活动轮判定主逻辑、估算采样均不变）。usage.js V11
/// （会话条让位，用户反馈：会话累计条悬浮位置落入输入框内部、遮挡输
/// 入提示文字）：样式表为输入区容器 .chat-composer-region 追加
/// padding-bottom（COMPOSER_GAP_PX = 22px，输入框整体干净上移、无布
/// 局破坏），会话条 SESSION_BAR_BOTTOM_PX 96 → 2（贴近页面最底部，落
/// 在输入区上移腾出的空隙中；渲染与数据管线零改动）。usage.js V12
/// （会话条动态测量定位到输入框上沿之上 + 还原输入框位置 + resize 自
/// 适应，用户反馈：V11 统计条贴死窗口边缘、与 ZCode 留白风格不协调）：
/// 删除 V11 的 .chat-composer-region padding-bottom 规则与
/// COMPOSER_GAP_PX 常量（输入框还原原位），会话条每次渲染动态测量
/// 输入区容器 getBoundingClientRect().top，bottom = window.innerHeight
/// - top + SESSION_BAR_ABOVE_PX（6px），测量失败退回固定
/// SESSION_BAR_BOTTOM_PX（96px 兜底）；初始化追加 window resize 监听
/// （scheduleRender）自适应缩放；渲染与数据管线零改动）。usage.js V13
/// （会话条 DOM 挂载进输入区容器 + CSS 顶部留白定位，废弃动态测量。
/// 实机缺陷：V12 的"测 region.top → fixed bottom 计算"会压住输入框内
/// 第一行文字——测量目标与可见输入卡片边缘不一致 + 输入框单行/多行
/// 切换的时序窗口）：ensureStyle 注入 .chat-composer-region relative +
/// padding-top:26px 顶部留白，会话条改 absolute（top:4px 居中）由
/// renderSessionBar 幂等挂载进容器，零坐标测量、删除 resize 监听（随
/// 文档流自动跟随）；region 缺失退回挂 body + fixed +
/// SESSION_BAR_BOTTOM_PX（96 兜底）；渲染与数据管线零改动）
/// 实机缺陷：V13 遗漏 SEL_COMPOSER 常量定义，挂载块每次抛
/// ReferenceError 被 catch 静默吞掉，会话条永远走 body + fixed 兜底。
/// V14 修复：常量区补回定义 + 挂载 catch 增加一次性告警。
/// V15：会话条 Σ 段新增会话总 Token（tin+tout+tcr），速度段去 ⋯ 前缀，
/// 流式估算段 ↓ ~ 改 ≈ 前缀（渲染与数据管线零改动）。
/// V16：整体移除每轮统计行（完成态/进行中/启动窗口）的鼠标悬浮 title
/// 提示（titleOf/liveTitleOf/LIVE_TITLE/LIVE_START_TITLE 一并删除，
/// 行可见内容与数据管线零改动）。
pub const USAGE_JS_VERSION: u32 = 16;

/// 版本标记的头部查找范围（字符数）：版本注释固定在文件头部，
/// 限定查找范围避免误匹配正文中的同名字样。
const VERSION_HEAD_CHARS: usize = 512;

/// 从模板/落盘文件内容头部提取版本号（"ZBAR-THEME-V" + 数字）。
/// 旧版文件无版本标记时返回 None（视为需要升级）。
pub(crate) fn template_version_of(text: &str) -> Option<u32> {
    let head: String = text.chars().take(VERSION_HEAD_CHARS).collect();
    let marker = "ZBAR-THEME-V";
    let pos = head.find(marker)?;
    let digits: String = head[pos + marker.len()..]
        .chars()
        .take_while(|c| c.is_ascii_digit())
        .collect();
    digits.parse().ok()
}

/// 版本化落盘单个模板文件：
/// - 文件不存在 → 写入内置模板；
/// - 无版本标记（旧版）或版本低于内置版本 → 覆盖升级；
/// - 版本不低于内置版本（可能已被实机调优）→ 不动。
pub(crate) fn ensure_versioned_template(
    path: &Path,
    template: &str,
    version: u32,
) -> Result<(), String> {
    let outdated = match fs::read_to_string(path) {
        Ok(text) => match template_version_of(&text) {
            Some(v) => v < version,
            None => true,
        },
        Err(_) => true,
    };
    if outdated {
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| path.to_string_lossy().to_string());
        fs::write(path, template).map_err(|e| format!("写入 {name} 失败: {e}"))?;
    }
    Ok(())
}

/// 确保/升级主题资产：
/// - theme.css / effects.js / usage.js：版本化覆盖（头部版本低于内置模板
///   时升级，见 ensure_versioned_template；实机调优过的当前版本不动）
/// - variables.css：按当前参数重渲（内容无变化时跳过写盘）
/// - 默认壁纸：wallpapers/ 无 default.mp4 且应用打包资源里有则拷入
///
/// 调用时机：安装主流程（携带打包资源壁纸）；皮肤页状态查询与参数/
/// 壁纸保存入口（传 None，仅做廉价的版本升级检查与重渲）——参数变化
/// 经 variables.css 每秒热重载即时生效，模板文件升级经"重启 ZCode"
/// 完全重载生效。
pub fn ensure_theme_assets(app_id: &str, resource_wallpapers: Option<&Path>) -> Result<(), String> {
    let dir = app_dir(app_id)?;
    let wp_dir = wallpapers_dir(app_id)?;
    ensure_theme_assets_in(&dir, &wp_dir, resource_wallpapers)
}

/// ensure_theme_assets 的目录显式版（单元测试复用，不依赖真实 ~/.zbar）。
pub(crate) fn ensure_theme_assets_in(
    dir: &Path,
    wp_dir: &Path,
    resource_wallpapers: Option<&Path>,
) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| format!("创建主题目录失败: {e}"))?;
    fs::create_dir_all(wp_dir).map_err(|e| format!("创建壁纸目录失败: {e}"))?;

    ensure_versioned_template(&dir.join(THEME_CSS), inject::THEME_CSS, THEME_CSS_VERSION)?;
    ensure_versioned_template(&dir.join(EFFECTS_JS), inject::EFFECTS_JS, EFFECTS_JS_VERSION)?;
    ensure_versioned_template(&dir.join(USAGE_JS), inject::USAGE_JS, USAGE_JS_VERSION)?;

    // 默认壁纸：优先应用打包资源（Tauri resources wallpapers/*），
    // 并行智能体产出的 default.mp4 会随应用分发；资源缺失时静默跳过，
    // 用户可后续通过 set_agent_wallpaper 手动导入。
    let target = wp_dir.join(DEFAULT_WALLPAPER_FILE);
    if !target.exists() {
        if let Some(res) = resource_wallpapers {
            let src = res.join(DEFAULT_WALLPAPER_FILE);
            if src.is_file() {
                fs::copy(&src, &target).map_err(|e| format!("拷贝默认壁纸失败: {e}"))?;
            }
        }
    }

    refresh_variables_css_in(dir)
}

/// 清理主题目录但保留 wallpapers/（卸载还原后调用：壁纸素材属于用户数据）。
pub fn cleanup_theme_dir_keep_wallpapers(app_id: &str) -> Result<(), String> {
    let dir = app_dir(app_id)?;
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(&dir).map_err(|e| format!("读取主题目录失败: {e}"))? {
        let entry = entry.map_err(|e| format!("读取主题目录失败: {e}"))?;
        if entry.file_name() == WALLPAPERS_DIR {
            continue;
        }
        let path = entry.path();
        let removed = if path.is_dir() {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        removed.map_err(|e| format!("清理主题目录失败（{}）: {e}", path.display()))?;
    }
    Ok(())
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 测试专用临时目录（每次唯一，测试结束统一清理）
    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-agent-theme-store-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn params_默认值读写() {
        let dir = test_dir("params-default");
        let path = dir.join(PARAMS_FILE);

        // 不存在时读默认
        assert!(read_params_file(&path).is_none());

        // 默认值写出 → 读回应完全一致（含 wallpaperFile 默认 default.mp4）
        let default = ThemeParams::default();
        assert_eq!(default.wp_brightness, DEFAULT_WP_BRIGHTNESS);
        assert_eq!(default.wp_saturate, DEFAULT_WP_SATURATE);
        assert_eq!(default.wp_blur, DEFAULT_WP_BLUR);
        assert_eq!(default.mask_strength, DEFAULT_MASK_STRENGTH);
        assert_eq!(default.panel_opacity, DEFAULT_PANEL_OPACITY);
        assert_eq!(default.sidebar_opacity, DEFAULT_SIDEBAR_OPACITY);
        assert_eq!(
            default.sidebar_right_opacity,
            DEFAULT_SIDEBAR_RIGHT_OPACITY
        );
        assert_eq!(default.playback_rate, DEFAULT_PLAYBACK_RATE);
        assert_eq!(default.base_alpha, DEFAULT_BASE_ALPHA);
        assert_eq!(default.text_shadow, DEFAULT_TEXT_SHADOW);
        assert_eq!(default.usage_font_size, DEFAULT_USAGE_FONT_SIZE);
        assert_eq!(default.usage_opacity, DEFAULT_USAGE_OPACITY);
        assert_eq!(default.usage_session_bar, DEFAULT_USAGE_SESSION_BAR);
        assert_eq!(default.wallpaper_file.as_deref(), Some(DEFAULT_WALLPAPER_FILE));
        assert_eq!(default.wallpaper_dir, None);

        write_params_file(&path, &default).unwrap();
        assert_eq!(read_params_file(&path), Some(default));

        // camelCase 序列化：前端契约字段名一字不差
        let text = fs::read_to_string(&path).unwrap();
        for key in [
            "wpBrightness", "wpSaturate", "wpBlur", "maskStrength",
            "panelOpacity", "sidebarOpacity", "sidebarRightOpacity",
            "playbackRate", "baseAlpha", "textShadow",
            "usageFontSize", "usageOpacity", "usageSessionBar",
            "wallpaperFile", "wallpaperDir",
        ] {
            assert!(text.contains(key), "params.json 缺少字段 {key}");
        }

        // 修改后读写
        let mut p = ThemeParams::default();
        p.wp_brightness = 1.0;
        p.wallpaper_file = Some("my.mp4".into());
        p.wallpaper_dir = Some("/Users/x/Pictures/wallpapers".into());
        write_params_file(&path, &p).unwrap();
        assert_eq!(read_params_file(&path), Some(p));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn params_缺字段按默认补齐() {
        let dir = test_dir("params-partial");
        let path = dir.join(PARAMS_FILE);
        // 旧版文件只有两个字段：其余应回默认
        fs::write(&path, r#"{"wpBrightness":0.9,"wallpaperFile":"a.mp4"}"#).unwrap();
        let p = read_params_file(&path).unwrap();
        assert_eq!(p.wp_brightness, 0.9);
        assert_eq!(p.wallpaper_file.as_deref(), Some("a.mp4"));
        assert_eq!(p.mask_strength, DEFAULT_MASK_STRENGTH);
        assert_eq!(p.sidebar_right_opacity, DEFAULT_SIDEBAR_RIGHT_OPACITY);
        assert_eq!(p.playback_rate, DEFAULT_PLAYBACK_RATE);
        // V10 新增参数：旧版文件缺字段同样按默认补齐
        assert_eq!(p.base_alpha, DEFAULT_BASE_ALPHA);
        assert_eq!(p.text_shadow, DEFAULT_TEXT_SHADOW);
        // V4 用量统计条参数：旧版 params.json 缺字段同样按默认补齐
        assert_eq!(p.usage_font_size, DEFAULT_USAGE_FONT_SIZE);
        assert_eq!(p.usage_opacity, DEFAULT_USAGE_OPACITY);
        // V5 会话累计条开关：旧版 params.json 缺字段按默认开启补齐
        assert_eq!(p.usage_session_bar, DEFAULT_USAGE_SESSION_BAR);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn usage_session_bar_开关序列化与兼容() {
        let dir = test_dir("usage-session-bar");
        let path = dir.join(PARAMS_FILE);

        // 显式关闭 → 落盘读回保持 false（不被默认值 true 覆盖），
        // camelCase 键名落盘
        let mut p = ThemeParams::default();
        p.usage_session_bar = false;
        write_params_file(&path, &p).unwrap();
        let back = read_params_file(&path).unwrap();
        assert!(!back.usage_session_bar, "显式关闭的开关读回应保持 false");
        let text = fs::read_to_string(&path).unwrap();
        assert!(
            text.contains("\"usageSessionBar\": false"),
            "params.json 应含 camelCase 键 usageSessionBar 且值为 false：{text}"
        );
        // clamp 收敛只针对数值/文本参数，开关布尔值原样保留
        assert!(!back.clamped().usage_session_bar, "clamped 不应改动开关值");

        // 旧版 params.json 缺该字段 → serde default 补默认值 true
        fs::write(&path, r#"{"wpBrightness":0.9,"wallpaperFile":"a.mp4"}"#).unwrap();
        let legacy = read_params_file(&path).unwrap();
        assert!(
            legacy.usage_session_bar,
            "旧版文件缺 usageSessionBar 应按默认开启补齐"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn params_越界值被收敛() {
        let mut p = ThemeParams {
            wp_brightness: 9.0,
            wp_saturate: -1.0,
            wp_blur: 999.0,
            mask_strength: 5.0,
            panel_opacity: 2.0,
            sidebar_opacity: -3.0,
            sidebar_right_opacity: 9.0,
            playback_rate: 10.0,
            base_alpha: 2.0,
            text_shadow: -1.0,
            usage_font_size: 99.0,
            usage_opacity: 5.0,
            usage_session_bar: false,
            wallpaper_file: Some("  ".into()),
            wallpaper_dir: None,
        }
        .clamped();
        assert_eq!(p.wp_brightness, WP_BRIGHTNESS_RANGE.1);
        assert_eq!(p.wp_saturate, WP_SATURATE_RANGE.0);
        assert_eq!(p.wp_blur, WP_BLUR_RANGE.1);
        assert_eq!(p.mask_strength, MASK_STRENGTH_RANGE.1);
        assert_eq!(p.panel_opacity, PANEL_OPACITY_RANGE.1);
        assert_eq!(p.sidebar_opacity, SIDEBAR_OPACITY_RANGE.0);
        assert_eq!(p.sidebar_right_opacity, SIDEBAR_RIGHT_OPACITY_RANGE.1);
        assert_eq!(p.playback_rate, PLAYBACK_RATE_RANGE.1);
        assert_eq!(p.base_alpha, BASE_ALPHA_RANGE.1);
        assert_eq!(p.text_shadow, TEXT_SHADOW_RANGE.0);
        assert_eq!(p.usage_font_size, USAGE_FONT_SIZE_RANGE.1);
        assert_eq!(p.usage_opacity, USAGE_OPACITY_RANGE.1);
        // 空白文件名回默认
        assert_eq!(p.wallpaper_name(), DEFAULT_WALLPAPER_FILE);
        p.wallpaper_file = None;
        assert_eq!(p.wallpaper_name(), DEFAULT_WALLPAPER_FILE);
    }

    #[test]
    fn 壁纸指向切换_更新params并重渲css() {
        let dir = test_dir("wallpaper-apply");
        let wp_dir = dir.join(WALLPAPERS_DIR);
        fs::create_dir_all(&wp_dir).unwrap();
        fs::write(wp_dir.join(DEFAULT_WALLPAPER_FILE), b"v").unwrap();
        fs::write(wp_dir.join("my.mp4"), b"v").unwrap();

        // 无 params.json（首装后未动过设置）→ 从默认值切到 my.mp4：
        // 指向落盘 + variables.css 重渲均指向新文件
        apply_wallpaper_in(&dir, "my.mp4").unwrap();
        let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap();
        assert_eq!(params.wallpaper_file.as_deref(), Some("my.mp4"));
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("my.mp4"), "variables.css 应指向新壁纸：{css}");
        assert!(
            !css.contains(DEFAULT_WALLPAPER_FILE),
            "不应再指向旧壁纸 default.mp4：{css}"
        );

        // 已有自定义参数时切回默认壁纸：仅指向更新，其余参数保持
        let mut p = ThemeParams::default();
        p.wp_brightness = 0.66;
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        apply_wallpaper_in(&dir, DEFAULT_WALLPAPER_FILE).unwrap();
        let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap();
        assert_eq!(params.wp_brightness, 0.66);
        assert_eq!(params.wallpaper_file.as_deref(), Some(DEFAULT_WALLPAPER_FILE));
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("default.mp4"), "css 应指回默认壁纸：{css}");
        assert!(css.contains("--zbar-wp-brightness: 0.66;"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn state_读写与安装中判定() {
        let dir = test_dir("state");
        let path = dir.join(STATE_FILE);

        // 默认态
        let s = StoredState::default();
        assert!(!s.is_installed());
        assert!(!s.is_installing_recent());

        // installing 且时间戳新鲜 → 拦截
        let mut s = StoredState {
            status: Some(STATUS_INSTALLING.to_string()),
            installing_since: Some(chrono::Utc::now().timestamp()),
            ..Default::default()
        };
        assert!(s.is_installing_recent());

        // installing 但时间戳超时 → 视为残留放行
        s.installing_since = Some(chrono::Utc::now().timestamp() - INSTALLING_STALE_SECS - 1);
        assert!(!s.is_installing_recent());

        // installed 状态序列化 → 反序列化保持
        let s = StoredState {
            status: Some(STATUS_INSTALLED.to_string()),
            zcode_version: Some("1.2.3".into()),
            asar_size: Some(284_000_000),
            asar_mtime: Some(1_770_000_000),
            injected_at: Some("2026-08-27T00:00:00+00:00".into()),
            injected_marker: true,
            installing_since: None,
        };
        let json = serde_json::to_string(&s).unwrap();
        fs::write(&path, &json).unwrap();
        let back = read_state_file(&path).unwrap();
        assert_eq!(back.zcode_version.as_deref(), Some("1.2.3"));
        assert_eq!(back.asar_size, Some(284_000_000));
        assert_eq!(back.asar_mtime, Some(1_770_000_000));
        assert!(back.injected_marker);
        assert!(back.is_installed());
        // camelCase 字段名
        assert!(json.contains("\"zcodeVersion\""));
        assert!(json.contains("\"asarSize\""));
        assert!(json.contains("\"asarMtime\""));
        assert!(json.contains("\"injectedMarker\""));

        // 旧版 state.json 缺 asarMtime → 按容器级 default 反序列化为 None
        fs::write(
            &path,
            r#"{"status":"installed","asarSize":284000000,"injectedMarker":true}"#,
        )
        .unwrap();
        let legacy = read_state_file(&path).unwrap();
        assert_eq!(legacy.asar_size, Some(284_000_000));
        assert_eq!(legacy.asar_mtime, None);
        assert!(legacy.injected_marker);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 模板版本_提取() {
        // 内置模板头部带各自的当前版本标记
        assert_eq!(template_version_of(inject::THEME_CSS), Some(THEME_CSS_VERSION));
        assert_eq!(template_version_of(inject::EFFECTS_JS), Some(EFFECTS_JS_VERSION));
        assert_eq!(template_version_of(inject::USAGE_JS), Some(USAGE_JS_VERSION));
        // 旧版文件无标记 → None（视为需要升级）
        assert_eq!(template_version_of("/* 旧版无版本头 */\nbody{}"), None);
        // 显式旧版本号可被提取
        assert_eq!(template_version_of("/* ZBAR-THEME-V1 */"), Some(1));
        // 版本标记在头部范围之外 → 不识别（防正文误匹配）
        let far = format!("{}ZBAR-THEME-V9", " ".repeat(600));
        assert_eq!(template_version_of(&far), None);
    }

    #[test]
    fn ensure_版本化升级_旧覆盖新不覆盖且用户数据不动() {
        let dir = test_dir("ensure-versioned");
        let wp_dir = dir.join(WALLPAPERS_DIR);

        // 预置用户数据：自定义参数 + 自定义壁纸（升级前后都必须原样保留）
        let mut p = ThemeParams::default();
        p.wp_brightness = 0.66;
        p.wallpaper_file = Some("mine.mp4".into());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        fs::create_dir_all(&wp_dir).unwrap();
        fs::write(wp_dir.join("mine.mp4"), b"mine-video").unwrap();

        // 场景一：旧版模板（无版本头）→ theme.css 升 V10、effects.js 升 V5
        fs::write(dir.join(THEME_CSS), "/* 旧版 theme，无版本头 */").unwrap();
        fs::write(dir.join(EFFECTS_JS), "// 旧版 effects，无版本头").unwrap();
        // usage.js 为本功能新增模板：老用户目录里不存在 → 首次 ensure 落盘
        ensure_theme_assets_in(&dir, &wp_dir, None).unwrap();
        assert!(
            fs::read_to_string(dir.join(THEME_CSS))
                .unwrap()
                .contains("ZBAR-THEME-V11"),
            "旧版 theme.css 应被升级覆盖到 V11"
        );
        assert!(
            fs::read_to_string(dir.join(EFFECTS_JS))
                .unwrap()
                .contains("ZBAR-THEME-V5"),
            "旧版 effects.js 应被升级覆盖到 V5"
        );
        assert!(
            fs::read_to_string(dir.join(USAGE_JS))
                .unwrap()
                .contains(&format!("ZBAR-THEME-V{USAGE_JS_VERSION}")),
            "缺失的 usage.js 应首次落盘"
        );

        // 场景一补充：usage.js 为旧版本（头部标记低于内置版本）→ 覆盖升级
        fs::write(dir.join(USAGE_JS), "// ZBAR-THEME-V0 旧版占位\n").unwrap();
        ensure_theme_assets_in(&dir, &wp_dir, None).unwrap();
        assert!(
            fs::read_to_string(dir.join(USAGE_JS))
                .unwrap()
                .contains(&format!("ZBAR-THEME-V{USAGE_JS_VERSION}")),
            "旧版 usage.js 应被升级覆盖"
        );

        // 场景二：effects.js 为 V3（已装用户的真实升级路径，V3/V4 旧版
        // 均应升到当前版本）→ 仅它升 V5
        fs::write(dir.join(THEME_CSS), inject::THEME_CSS).unwrap();
        fs::write(
            dir.join(EFFECTS_JS),
            "// ZBAR-THEME-V3 旧版无 theme.css 热重载\n",
        )
        .unwrap();
        ensure_theme_assets_in(&dir, &wp_dir, None).unwrap();
        assert!(
            fs::read_to_string(dir.join(EFFECTS_JS))
                .unwrap()
                .contains("ZBAR-THEME-V5"),
            "V3 effects.js 应被升级到 V5"
        );

        // 场景三：已是当前版本（用户实机调优过，头部追加了自定义内容）→ 不覆盖
        let tuned_theme = format!("{}\n/* 用户实机调优追加 */\n", inject::THEME_CSS);
        let tuned_effects = format!("{}\n// 用户实机调优追加\n", inject::EFFECTS_JS);
        fs::write(dir.join(THEME_CSS), &tuned_theme).unwrap();
        fs::write(dir.join(EFFECTS_JS), &tuned_effects).unwrap();
        ensure_theme_assets_in(&dir, &wp_dir, None).unwrap();
        assert_eq!(
            fs::read_to_string(dir.join(THEME_CSS)).unwrap(),
            tuned_theme,
            "当前版本文件不应被覆盖"
        );
        assert_eq!(
            fs::read_to_string(dir.join(EFFECTS_JS)).unwrap(),
            tuned_effects,
            "当前版本文件不应被覆盖"
        );

        // 用户数据不受升级影响：参数与壁纸原样，variables.css 按用户参数渲染
        let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap();
        assert_eq!(params.wp_brightness, 0.66);
        assert_eq!(params.wallpaper_file.as_deref(), Some("mine.mp4"));
        assert_eq!(fs::read(wp_dir.join("mine.mp4")).unwrap(), b"mine-video");
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("mine.mp4"), "variables.css 应指向用户壁纸：{css}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn variables_css_重渲幂等_内容不变不写盘() {
        let dir = test_dir("refresh-idempotent");
        refresh_variables_css_in(&dir).unwrap();
        let path = dir.join(VARIABLES_CSS);
        let content = fs::read_to_string(&path).unwrap();
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();

        // 内容无变化 → 跳过写盘（mtime 不变）
        std::thread::sleep(std::time::Duration::from_millis(50));
        refresh_variables_css_in(&dir).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), content);
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            mtime,
            "内容未变时不应重写文件"
        );

        // 参数变化 → 重渲写盘
        let mut p = ThemeParams::default();
        p.wp_brightness = 0.5;
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        refresh_variables_css_in(&dir).unwrap();
        assert!(fs::read_to_string(&path).unwrap().contains("--zbar-wp-brightness: 0.5;"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn wallpaper_file_绝对路径与相对名渲染() {
        let dir = test_dir("wp-ref-semantics");
        let wp_dir = dir.join(WALLPAPERS_DIR);
        fs::create_dir_all(&wp_dir).unwrap();
        fs::write(wp_dir.join("rel.mp4"), b"v").unwrap();
        let external = dir.join("外部图片.png");
        fs::write(&external, b"i").unwrap();

        // 相对名：拼 wallpapers/ 目录
        let mut p = ThemeParams::default();
        p.wallpaper_file = Some("rel.mp4".into());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        refresh_variables_css_in(&dir).unwrap();
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains(&inject::file_url(&wp_dir.join("rel.mp4"))));
        assert!(css.contains("rel.mp4"));

        // 绝对路径（含中文与空格）：直接引用，不拼 wallpapers/
        p.wallpaper_file = Some(external.to_string_lossy().to_string());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        refresh_variables_css_in(&dir).unwrap();
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(
            css.contains(&inject::file_url(&external)),
            "绝对路径应直接 file_url 引用：{css}"
        );
        assert!(
            !css.contains("wallpapers"),
            "绝对路径不应拼 wallpapers/ 目录：{css}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 壁纸选择_路径校验与指向切换() {
        let dir = test_dir("wp-select");
        let wp_dir = dir.join(WALLPAPERS_DIR);
        fs::create_dir_all(&wp_dir).unwrap();
        fs::write(wp_dir.join("inner.mp4"), b"v").unwrap();
        // 用户壁纸目录（含子目录文件）
        let user_dir = dir.join("user-walls");
        fs::create_dir_all(user_dir.join("sub")).unwrap();
        fs::write(user_dir.join("pic.jpg"), b"i").unwrap();
        fs::write(user_dir.join("sub/deep.webp"), b"i").unwrap();
        // 白名单外的文件（目录外）
        let outside = dir.join("secret.txt");
        fs::write(&outside, b"s").unwrap();
        let outside_mp4 = dir.join("outside.mp4");
        fs::write(&outside_mp4, b"v").unwrap();

        let mut p = ThemeParams::default();
        p.wallpaper_dir = Some(user_dir.canonicalize().unwrap().to_string_lossy().to_string());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();

        // 目录外文件：即使类型合法也拒绝（防任意路径注入）
        let err = resolve_selectable_wallpaper_in(
            &dir,
            &wp_dir,
            &outside_mp4.to_string_lossy().to_string(),
        )
        .expect_err("目录外的合法类型文件应被拒绝");
        assert!(err.contains("壁纸目录内"), "错误应说明白名单约束：{err}");
        // 不受支持的类型（目录外造一个 txt）
        let err = resolve_selectable_wallpaper_in(
            &dir,
            &wp_dir,
            &outside.to_string_lossy().to_string(),
        )
        .expect_err("不受支持类型应被拒绝");
        assert!(err.contains("仅支持"), "错误应说明支持的类型：{err}");
        // 路径遍历串：文件不存在直接拒绝
        let err = resolve_selectable_wallpaper_in(&dir, &wp_dir, "default.mp4../../etc/passwd")
            .expect_err("路径遍历应被拒绝");
        assert!(err.contains("不存在"), "错误应说明文件不存在：{err}");
        // 不受支持的扩展名（在用户目录内造一个）
        fs::write(user_dir.join("note.txt"), b"x").unwrap();
        let err = resolve_selectable_wallpaper_in(
            &dir,
            &wp_dir,
            &user_dir.join("note.txt").to_string_lossy().to_string(),
        )
        .expect_err("目录内不受支持类型同样应被拒绝");
        assert!(err.contains("仅支持"), "错误应说明支持的类型：{err}");

        // 目录内文件：wallpapers/ 与用户目录（含子目录）均可选中，
        // params 存 canonicalize 后的绝对路径并重渲 variables.css
        let inner = wp_dir.join("inner.mp4");
        select_wallpaper_in(&dir, &wp_dir, &inner.to_string_lossy()).unwrap();
        let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap();
        let expected_inner = inject::strip_verbatim_prefix(inner.canonicalize().unwrap())
            .to_string_lossy()
            .to_string();
        assert_eq!(params.wallpaper_file.as_deref(), Some(expected_inner.as_str()));
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("inner.mp4"), "variables.css 应指向选中壁纸：{css}");

        let deep = user_dir.join("sub/deep.webp");
        select_wallpaper_in(&dir, &wp_dir, &deep.to_string_lossy()).unwrap();
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("deep.webp"));

        // "default" → 指向回落 default.mp4（相对名语义）
        select_wallpaper_in(&dir, &wp_dir, "default").unwrap();
        let params = read_params_file(&dir.join(PARAMS_FILE)).unwrap();
        assert_eq!(params.wallpaper_file.as_deref(), Some(DEFAULT_WALLPAPER_FILE));
        // 其余参数（含 wallpaper_dir）不受切换影响
        assert!(params.wallpaper_dir.is_some());
        let css = fs::read_to_string(dir.join(VARIABLES_CSS)).unwrap();
        assert!(css.contains("default.mp4"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_backup_meta驱动优先于mtime() {
        let dir = test_dir("latest-meta-driven");
        let bak_dir = dir.join(BACKUP_DIR);
        fs::create_dir_all(&bak_dir).unwrap();

        // meta 指向 v1.0.0.100（先写入，mtime 更早）；
        // 另造一个 mtime 更新的 v2.0.0.999（真机事故形态：旧版本 .bak
        // 因重装触碰反而 mtime 更新）。
        // meta 是"最后一次成功备份"的权威记录 → 必须选中 v1.0.0 那份。
        write_backup_meta_in(&bak_dir, 100, Some("1.0.0".into())).unwrap();
        let meta_target = bak_dir.join(backup_file_name(Some("1.0.0"), 100));
        fs::write(&meta_target, b"pristine-v1").unwrap();
        // 间隔确保 mtime 严格更新（不同文件系统时间戳精度不一）
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newer = bak_dir.join(backup_file_name(Some("2.0.0"), 999));
        fs::write(&newer, b"old-version-fresh-mtime").unwrap();

        assert_eq!(
            latest_backup_in(&bak_dir),
            Some(meta_target),
            "meta 指向的备份应被选中，即使其他 .bak 的 mtime 更新"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn latest_backup_meta缺失或指向缺失时mtime兜底() {
        let dir = test_dir("latest-meta-fallback");
        let bak_dir = dir.join(BACKUP_DIR);
        fs::create_dir_all(&bak_dir).unwrap();

        // 场景一：无 meta.json → 按 mtime 取最新
        fs::write(bak_dir.join("app.asar.v1.0.0.100.bak"), b"a").unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        let newer = bak_dir.join("app.asar.v2.0.0.200.bak");
        fs::write(&newer, b"b").unwrap();
        assert_eq!(latest_backup_in(&bak_dir), Some(newer.clone()));

        // 场景二：meta 指向的文件不存在（被外部删除/清理）→ 同样回落 mtime
        write_backup_meta_in(&bak_dir, 300, Some("3.0.0".into())).unwrap();
        assert_eq!(
            latest_backup_in(&bak_dir),
            Some(newer),
            "meta 指向缺失时应降级为 mtime 兜底"
        );

        // 场景三：目录里既无 meta 也无 .bak → None
        let empty = dir.join("empty-backup");
        fs::create_dir_all(&empty).unwrap();
        assert_eq!(latest_backup_in(&empty), None);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 旧版本备份清理_保留当前删除其他() {
        let dir = test_dir("remove-stale-backups");
        let bak_dir = dir.join(BACKUP_DIR);
        fs::create_dir_all(&bak_dir).unwrap();

        let keep = bak_dir.join(backup_file_name(Some("3.10.0"), 297625072));
        fs::write(&keep, b"current").unwrap();
        let stale_a = bak_dir.join(backup_file_name(Some("3.9.2"), 307008658));
        fs::write(&stale_a, b"stale-a").unwrap();
        let stale_b = bak_dir.join("app.asar.vunknown.123.bak");
        fs::write(&stale_b, b"stale-b").unwrap();
        // 非 .bak 备份内容（meta.json 等）一律不动
        write_backup_meta_in(&bak_dir, 297625072, Some("3.10.0".into())).unwrap();
        let unrelated = bak_dir.join("other.txt");
        fs::write(&unrelated, b"keep-me").unwrap();

        remove_stale_backups_in(&bak_dir, &keep).unwrap();

        assert!(keep.is_file(), "meta 指向的当前备份必须保留");
        assert!(!stale_a.exists(), "旧版本备份应被删除（每个约 300MB，纯占空间）");
        assert!(!stale_b.exists(), "unknown 版本的旧备份同样应被删除");
        assert!(bak_dir.join(BACKUP_META_FILE).is_file(), "meta.json 不应被动");
        assert!(unrelated.is_file(), "非备份文件不应被动");

        // keep 不在目录内 / 目录不存在 → 幂等不报错
        remove_stale_backups_in(&bak_dir, &dir.join("elsewhere.bak")).unwrap();
        remove_stale_backups_in(&dir.join("no-such-dir"), &keep).unwrap();

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn backup_file_name_非法字符替换() {
        assert_eq!(backup_file_name(Some("1.2.3"), 284), "app.asar.v1.2.3.284.bak");
        assert_eq!(backup_file_name(None, 100), "app.asar.vunknown.100.bak");
        let weird = backup_file_name(Some("1/0:2"), 5);
        assert_eq!(weird, "app.asar.v1-0-2.5.bak");
    }

    #[test]
    fn 壁纸列表_互斥聚合与默认项过滤() {
        let dir = test_dir("wp-list");
        let wp_dir = dir.join(WALLPAPERS_DIR);
        // 内置目录：平铺（含默认项、隐藏文件与不支持类型，默认项应被过滤）
        fs::create_dir_all(&wp_dir).unwrap();
        fs::write(wp_dir.join(DEFAULT_WALLPAPER_FILE), b"v").unwrap();
        fs::write(wp_dir.join("zeta.mp4"), b"v").unwrap();
        fs::write(wp_dir.join("alpha.png"), b"i").unwrap();
        fs::write(wp_dir.join(".DS_Store"), b"x").unwrap();
        fs::write(wp_dir.join("readme.txt"), b"x").unwrap();
        // 用户目录：递归子目录 + 隐藏子目录
        let user_dir = dir.join("user-walls");
        fs::create_dir_all(user_dir.join("nested/deep")).unwrap();
        fs::create_dir_all(user_dir.join(".hidden")).unwrap();
        fs::write(user_dir.join("mid.jpg"), b"i").unwrap();
        fs::write(user_dir.join("nested/deep/beta.webm"), b"v").unwrap();
        fs::write(user_dir.join(".hidden/nope.png"), b"i").unwrap();

        // 场景一：未设置 wallpaper_dir → 仅内置目录（默认项由 DTO 补首，
        // 清单内的 default.mp4 实体必须被过滤，否则重复成两张卡片）
        let names = |files: &[PathBuf]| -> Vec<String> {
            files
                .iter()
                .map(|f| f.file_name().unwrap().to_string_lossy().to_string())
                .collect()
        };
        let files = list_wallpapers_in(&dir, &wp_dir);
        assert_eq!(
            names(&files),
            vec!["alpha.png", "zeta.mp4"],
            "仅内置目录、按文件名排序且不含 default.mp4"
        );

        // 场景二：设置 wallpaper_dir → 互斥聚合：仅用户目录（全递归），
        // 不再混入内置 wallpapers/ 的旧内容
        let mut p = ThemeParams::default();
        p.wallpaper_dir = Some(user_dir.to_string_lossy().to_string());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        let files = list_wallpapers_in(&dir, &wp_dir);
        assert_eq!(
            names(&files),
            vec!["beta.webm", "mid.jpg"],
            "设置目录后仅返回用户目录内容，不含内置 wallpapers/"
        );
        assert!(
            files.iter().any(|f| f.ends_with("nested/deep/beta.webm")),
            "用户目录子目录文件应被递归收录"
        );

        // 场景三：目录不存在 → 回落内置 wallpapers 聚合（同场景一口径）
        p.wallpaper_dir = Some(dir.join("no-such-dir").to_string_lossy().to_string());
        write_params_file(&dir.join(PARAMS_FILE), &p).unwrap();
        let files = list_wallpapers_in(&dir, &wp_dir);
        assert_eq!(
            names(&files),
            vec!["alpha.png", "zeta.mp4"],
            "目录不存在时回落内置目录且过滤 default.mp4"
        );

        let _ = fs::remove_dir_all(&dir);
    }
}

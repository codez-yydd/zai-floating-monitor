//! 额度预警阈值配置（~/.zbar/notify.json）。
//!
//! 仅负责配置的读写；进度条颜色按这些阈值在 QuotaPanel 前端渲染。
//! 不再有后台轮询 / 系统通知 / 菜单栏 🔴 —— 用户反馈那套打扰且不直观，
//! 改为在面板内用进度条颜色（绿/黄/红）直观呈现额度等级。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pricing::config_dir;

/// 额度预警配置（~/.zbar/notify.json）。
/// 三个阈值分别对应 5h 窗口 / 周额度 / MCP 月度。
/// 前端进度条颜色规则：百分比 < 阈值 → 绿；[阈值, 阈值+15) → 黄；≥ 阈值+15 → 红。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NotifyConfig {
    /// 总开关（保留字段，前端 UI 用；后端不再据此轮询）
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 5 小时窗口阈值（百分比），默认 80
    #[serde(default = "default_hour5")]
    pub hour5_threshold: u32,
    /// 周额度阈值（百分比），默认 85
    #[serde(default = "default_weekly")]
    pub weekly_threshold: u32,
    /// MCP 月度阈值（百分比），默认 80
    #[serde(default = "default_mcp")]
    pub mcp_threshold: u32,
}

fn default_enabled() -> bool {
    true
}
fn default_hour5() -> u32 {
    75
}
fn default_weekly() -> u32 {
    80
}
fn default_mcp() -> u32 {
    75
}

impl Default for NotifyConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            hour5_threshold: default_hour5(),
            weekly_threshold: default_weekly(),
            mcp_threshold: default_mcp(),
        }
    }
}

/// ~/.zbar/notify.json
fn notify_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("notify.json"))
}

/// 读取配置；文件不存在返回默认（不报错）。
pub fn load_notify() -> NotifyConfig {
    let path = match notify_path() {
        Ok(p) => p,
        Err(_) => return NotifyConfig::default(),
    };
    if !path.exists() {
        return NotifyConfig::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return NotifyConfig::default(),
    };
    serde_json::from_str::<NotifyConfig>(&data).unwrap_or_default()
}

/// 写入配置。
pub fn save_notify(cfg: &NotifyConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = notify_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化通知配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入通知配置失败: {e}"))
}

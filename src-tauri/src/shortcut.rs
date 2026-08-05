//! 全局快捷键配置：唤起面板的快捷键（可自定义，存 ~/.zbar/shortcut.json）。

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pricing::config_dir;

/// 默认快捷键：macOS 用 Option(Alt)+Shift+Z，跨平台统一用 alt+shift+z
const DEFAULT_SHORTCUT: &str = "alt+shift+z";

/// 快捷键配置
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutConfig {
    /// 是否启用
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 快捷键串，如 "alt+shift+z" / "ctrl+shift+z"（Tauri accelerator 格式）
    #[serde(default = "default_shortcut")]
    pub accelerator: String,
}

fn default_enabled() -> bool {
    true
}
fn default_shortcut() -> String {
    DEFAULT_SHORTCUT.to_string()
}

impl Default for ShortcutConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            accelerator: default_shortcut(),
        }
    }
}

/// ~/.zbar/shortcut.json
fn shortcut_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("shortcut.json"))
}

/// 读取快捷键配置；文件不存在返回默认。
pub fn load_shortcut() -> ShortcutConfig {
    let path = match shortcut_path() {
        Ok(p) => p,
        Err(_) => return ShortcutConfig::default(),
    };
    if !path.exists() {
        return ShortcutConfig::default();
    }
    let data = match std::fs::read_to_string(&path) {
        Ok(d) => d,
        Err(_) => return ShortcutConfig::default(),
    };
    serde_json::from_str::<ShortcutConfig>(&data).unwrap_or_default()
}

/// 写入快捷键配置。
pub fn save_shortcut(cfg: &ShortcutConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = shortcut_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化快捷键配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入快捷键配置失败: {e}"))
}

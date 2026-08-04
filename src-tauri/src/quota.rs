use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pricing::config_dir;

/// 端点选择："cn" = open.bigmodel.cn，"global" = api.z.ai
pub const ENDPOINT_CN: &str = "cn";
pub const ENDPOINT_GLOBAL: &str = "global";

/// 额度查询配置（用户手动填写 Token + 选择端点）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaConfig {
    /// Coding Plan 的 API Token
    #[serde(default)]
    pub token: String,
    /// "cn" | "global"
    #[serde(default = "default_endpoint")]
    pub endpoint: String,
}

fn default_endpoint() -> String {
    ENDPOINT_CN.to_string()
}

impl Default for QuotaConfig {
    fn default() -> Self {
        Self {
            token: String::new(),
            endpoint: default_endpoint(),
        }
    }
}

/// ~/.zbar/quota.json
pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("quota.json"))
}

/// 读取额度查询配置；文件不存在则返回默认空配置（不报错）。
pub fn load_quota() -> Result<QuotaConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(QuotaConfig::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取额度配置失败: {e}"))?;
    serde_json::from_str::<QuotaConfig>(&data)
        .map_err(|e| format!("解析额度配置失败: {e}"))
}

/// 写入额度查询配置。
pub fn save_quota(cfg: &QuotaConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化额度配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入额度配置失败: {e}"))
}

// ===== 额度接口返回结构 =====

/// 单条用量限制（与 BigModel 接口的 limits[] 元素对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaLimit {
    /// "TOKENS_LIMIT" | "TIME_LIMIT"
    #[serde(rename = "type")]
    pub kind: String,
    /// 接口窗口单位：3 = 小时，6 = 周。
    #[serde(default)]
    pub unit: u32,
    /// 窗口数量：5 小时窗口为 5，周窗口为 1。
    #[serde(default)]
    pub number: u32,
    /// 已用百分比（0-100）
    #[serde(default)]
    pub percentage: u32,
    /// 下次重置时间（毫秒时间戳，接口字段为驼峰 nextResetTime）
    #[serde(default, rename = "nextResetTime")]
    pub next_reset_time: Option<i64>,
}

/// BigModel 接口原始返回里的 data 字段
#[derive(Debug, Clone, Deserialize)]
struct QuotaData {
    #[serde(default)]
    limits: Vec<QuotaLimit>,
    /// 套餐等级："pro" / "max" ...
    #[serde(default)]
    level: String,
}

/// BigModel 接口原始返回
#[derive(Debug, Clone, Deserialize)]
struct QuotaResponse {
    #[serde(default)]
    data: Option<QuotaData>,
    #[serde(default)]
    msg: String,
    #[serde(default)]
    success: bool,
}

/// 解析后供前端使用的结果：把 limits 拆成「5小时」与「每周」两组
#[derive(Debug, Clone, Serialize)]
pub struct QuotaResult {
    /// 套餐等级
    pub level: String,
    /// 5小时窗口用量（已用百分比）
    pub hour5: Option<QuotaLimit>,
    /// 每周用量（已用百分比）
    pub weekly: Option<QuotaLimit>,
}

/// 根据 endpoint 选择 base URL
pub fn endpoint_base(endpoint: &str) -> &str {
    if endpoint == ENDPOINT_GLOBAL {
        "https://api.z.ai"
    } else {
        "https://open.bigmodel.cn"
    }
}

/// 请求额度接口并解析。
///
/// 接口返回的 limits 中包含多个类型和窗口，不能只按 nextResetTime 排序：
/// 5 小时窗口刚刷新后可能没有 nextResetTime，反而会被排序到最后。
pub fn fetch_quota(cfg: &QuotaConfig) -> Result<QuotaResult, String> {
    if cfg.token.trim().is_empty() {
        return Err("未配置 Token，请在设置中填写 Coding Plan API Token".into());
    }

    let base = endpoint_base(&cfg.endpoint);
    let url = format!("{base}/api/monitor/usage/quota/limit");

    let resp: QuotaResponse = ureq::get(&url)
        .set("Authorization", cfg.token.trim())
        .call()
        .map_err(|e| format!("请求额度接口失败: {e}"))?
        .into_json()
        .map_err(|e| format!("解析额度响应失败: {e}"))?;

    if !resp.success {
        return Err(if resp.msg.is_empty() {
            "额度接口返回失败".into()
        } else {
            resp.msg
        });
    }

    let data = resp.data.ok_or("额度响应缺少 data 字段")?;

    // 仅取 TOKENS_LIMIT。窗口类型由 unit + number 识别：
    // (3, 5) = 5 小时；(6, 1) = 每周。
    let token_limits: Vec<QuotaLimit> = data
        .limits
        .into_iter()
        .filter(|l| l.kind == "TOKENS_LIMIT")
        .collect();

    let hour5 = token_limits
        .iter()
        .find(|l| l.unit == 3 && l.number == 5)
        .cloned()
        // 兼容旧接口：刚刷新后的短窗口通常没有 nextResetTime。
        .or_else(|| {
            token_limits
                .iter()
                .find(|l| l.next_reset_time.is_none())
                .cloned()
        })
        .or_else(|| token_limits.first().cloned());

    let weekly = token_limits
        .iter()
        .find(|l| l.unit == 6 && l.number == 1)
        .cloned()
        .or_else(|| {
            token_limits
                .iter()
                .find(|l| {
                    hour5
                        .as_ref()
                        .map(|h| h.unit != l.unit || h.number != l.number)
                        .unwrap_or(true)
                })
                .cloned()
        });

    Ok(QuotaResult {
        level: data.level,
        hour5,
        weekly,
    })
}

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

/// MCP 工具用量明细（usageDetails[] 元素）
/// 仅 type=TIME_LIMIT（即 MCP 月度额度）会出现。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct McpUsageDetail {
    /// 工具代号：search-prime / web-reader / zread ...
    #[serde(default)]
    pub model_code: String,
    /// 该工具已用次数
    #[serde(default)]
    pub usage: i64,
}

/// 单条用量限制（与 BigModel 接口的 limits[] 元素对应）
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QuotaLimit {
    /// "TOKENS_LIMIT" | "TIME_LIMIT"（TIME_LIMIT 即 MCP 月度额度）
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
    /// MCP 已用次数（仅 TIME_LIMIT 有，接口字段 currentValue）
    #[serde(default, rename = "currentValue")]
    pub current_value: Option<i64>,
    /// MCP 总额度次数（仅 TIME_LIMIT 有；注意接口字段名是 usage，不是 total）
    #[serde(default)]
    pub usage: Option<i64>,
    /// MCP 按工具拆分明细（仅 TIME_LIMIT 有）
    #[serde(default, rename = "usageDetails")]
    pub usage_details: Option<Vec<McpUsageDetail>>,
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

/// 解析后供前端使用的结果：把 limits 拆成「5小时」「每周」「MCP 月度」三组
#[derive(Debug, Clone, Serialize)]
pub struct QuotaResult {
    /// 套餐等级
    pub level: String,
    /// 5小时窗口用量（已用百分比）
    pub hour5: Option<QuotaLimit>,
    /// 每周用量（已用百分比）
    pub weekly: Option<QuotaLimit>,
    /// MCP 月度用量（已用次数 + 总量 + 百分比）
    pub mcp: Option<QuotaLimit>,
}

/// 根据 endpoint 选择 base URL
pub fn endpoint_base(endpoint: &str) -> &str {
    if endpoint == ENDPOINT_GLOBAL {
        "https://api.z.ai"
    } else {
        "https://open.bigmodel.cn"
    }
}

/// 请求额度接口并解析（纯查询，不写快照）。
///
/// 接口返回的 limits 中包含多个类型和窗口，不能只按 nextResetTime 排序：
/// 5 小时窗口刚刷新后可能没有 nextResetTime，反而会被排序到最后。
///
/// 注意：本函数不写 quota_history 快照。仅前端 QuotaPanel 的主动刷新（fetch_quota）
/// 才写快照；其他调用方应使用本函数，避免高频轮询污染历史。
pub fn query_quota(cfg: &QuotaConfig) -> Result<QuotaResult, String> {
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

    // MCP 月度额度：type=TIME_LIMIT（与 token 额度区分开，先单独取）
    let mcp = data
        .limits
        .iter()
        .find(|l| l.kind == "TIME_LIMIT")
        .cloned();

    // token 额度（TOKENS_LIMIT），窗口类型由 unit + number 识别：
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
        mcp,
    })
}

/// 查询额度并写一条历史快照（供前端 fetch_quota 命令调用）。
pub fn fetch_quota(cfg: &QuotaConfig) -> Result<QuotaResult, String> {
    let result = query_quota(cfg)?;

    // 采样：每次成功查询追加一条快照（静默失败，不影响额度查询本身）。
    // 用本地时间作为采样 ts，与 model_usage.started_at (UTC) 保持同口径。
    let snap = crate::quota_history::QuotaSnapshot {
        ts: chrono::Local::now().timestamp_millis(),
        level: result.level.clone(),
        weekly_pct: result.weekly.as_ref().map(|w| w.percentage).unwrap_or(0),
        weekly_reset: result.weekly.as_ref().and_then(|w| w.next_reset_time),
        hour5_pct: result.hour5.as_ref().map(|h| h.percentage).unwrap_or(0),
        mcp_pct: result.mcp.as_ref().map(|m| m.percentage).unwrap_or(0),
        mcp_used: result.mcp.as_ref().and_then(|m| m.current_value),
        mcp_total: result.mcp.as_ref().and_then(|m| m.usage),
    };
    crate::quota_history::append_snapshot(&snap);

    Ok(result)
}

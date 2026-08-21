use serde::{Deserialize, Serialize};
use std::time::Duration;

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

/// 按选中 provider 的 baseURL 推断额度接口的 base：
/// 含 "z.ai" → https://api.z.ai；其余（含 bigmodel 或 baseURL 缺失为空串）→ 国内站。
fn base_from_provider_url(url: &str) -> &'static str {
    if url.contains("z.ai") {
        "https://api.z.ai"
    } else {
        "https://open.bigmodel.cn"
    }
}

// ===== ZCode 客户端凭证（只读）=====

/// 读取 ~/.zcode/v2/config.json 中登录 Coding Plan 后自动写入的凭证
/// （只读，绝不写回——该文件由 ZCode 客户端维护，外部写回极易把
/// ZCode 的登录态搞坏；key 的增删与刷新由 ZCode 客户端自行管理）。
/// 返回 (provider_key, api_key, base_url)，其中 base_url 取该 provider
/// 的 options.baseURL（用于推断额度接口端点，缺失时为空串）。
///
/// 错误文案统一以「未找到 ZCode Coding Plan 凭证」开头：前端 QuotaPanel /
/// SummaryTab 以该固定前缀识别登录引导分支（后端改前缀须与前端同步）。
fn pick_from_config() -> Result<(String, String, String), String> {
    let home = dirs::home_dir().ok_or(
        "未找到 ZCode Coding Plan 凭证：无法定位用户主目录，请先在 ZCode 客户端登录 Coding Plan 订阅",
    )?;
    let path = home.join(".zcode").join("v2").join("config.json");
    if !path.exists() {
        return Err("未找到 ZCode Coding Plan 凭证（~/.zcode/v2/config.json 不存在），请先在 ZCode 客户端登录 Coding Plan 订阅".into());
    }
    let data = std::fs::read_to_string(&path).map_err(|e| {
        format!("未找到 ZCode Coding Plan 凭证（读取 config.json 失败: {e}），请先在 ZCode 客户端登录 Coding Plan 订阅")
    })?;
    let root: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
        format!("未找到 ZCode Coding Plan 凭证（config.json 格式异常: {e}），请先在 ZCode 客户端登录 Coding Plan 订阅")
    })?;
    let providers = root.get("provider").and_then(|v| v.as_object()).ok_or(
        "未找到 ZCode Coding Plan 凭证（config.json 缺少 provider 配置），请先在 ZCode 客户端登录 Coding Plan 订阅",
    )?;
    pick_coding_plan_api_key(providers).ok_or(
        "未找到 ZCode Coding Plan 凭证（~/.zcode/v2/config.json），请先在 ZCode 客户端登录 Coding Plan 订阅"
            .into(),
    )
}

/// 从 config.json 顶层 provider map 中选出 Coding Plan 凭证（纯解析，便于单测），
/// 返回 (provider_key, api_key, base_url)。
/// 优先按内置顺序取 builtin:bigmodel-coding-plan / builtin:zai-coding-plan；
/// 其 apiKey 为空或 key 不存在时，回退到任意 key 含 "coding-plan" 且 apiKey
/// 非空的 provider（用户通常只登录一个订阅，回退天然命中实际登录方）。
/// 注意：builtin:bigmodel-start-plan / builtin:zai-start-plan 是轻量入门订阅，
/// 不可用于查询订阅额度，好在它们的 key 不含 "coding-plan" 子串，天然被回退排除。
fn pick_coding_plan_api_key(
    providers: &serde_json::Map<String, serde_json::Value>,
) -> Option<(String, String, String)> {
    // 单个 provider 的非空 apiKey（首尾空白去除）
    let non_empty_key = |v: &serde_json::Value| -> Option<String> {
        v.get("options")
            .and_then(|o| o.get("apiKey"))
            .and_then(|k| k.as_str())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.to_string())
    };

    // 单个 provider 的 baseURL（字符串值，缺失给空串，由 base_from_provider_url 兜底）
    let base_url_of = |v: &serde_json::Value| -> String {
        v.get("options")
            .and_then(|o| o.get("baseURL"))
            .and_then(|u| u.as_str())
            .unwrap_or("")
            .to_string()
    };

    // 内置 Coding Plan provider 固定优先顺序（无端点配置后的确定性选择）
    for preferred in ["builtin:bigmodel-coding-plan", "builtin:zai-coding-plan"] {
        if let Some(api_key) = providers.get(preferred).and_then(non_empty_key) {
            let base_url = providers
                .get(preferred)
                .map(base_url_of)
                .unwrap_or_default();
            return Some((preferred.to_string(), api_key, base_url));
        }
    }
    // 回退：任意 key 含 "coding-plan" 且 apiKey 非空（start-plan 不含该子串，被排除）
    providers
        .iter()
        .filter(|(k, _)| k.contains("coding-plan"))
        .find_map(|(k, v)| {
            non_empty_key(v).map(|key| (k.clone(), key, base_url_of(v)))
        })
}

/// 请求额度接口并解析（纯查询，不写快照）。
///
/// 接口返回的 limits 中包含多个类型和窗口，不能只按 nextResetTime 排序：
/// 5 小时窗口刚刷新后可能没有 nextResetTime，反而会被排序到最后。
///
/// 注意：本函数不写 quota_history 快照。仅前端 QuotaPanel 的主动刷新（fetch_quota）
/// 才写快照；其他调用方应使用本函数，避免高频轮询污染历史。
///
/// 凭证与接口端点均自动推断：读取 ZCode 客户端本地登录态选出的 provider，
/// 按其 options.baseURL 判断走 api.z.ai 还是 open.bigmodel.cn。
pub fn query_quota() -> Result<QuotaResult, String> {
    let (_provider_key, token, base_url) = pick_from_config()?;

    let base = base_from_provider_url(&base_url);
    let url = format!("{base}/api/monitor/usage/quota/limit");

    // 15s 请求总超时：ureq 默认无超时，网络异常时会无限等待卡死调用方；
    // 对齐其他模块的做法（cursor.rs 用 agent 级 30s 总超时，此处请求级 15s 即可）
    let resp: QuotaResponse = ureq::get(&url)
        .set("Authorization", token.trim())
        .timeout(Duration::from_secs(15))
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
pub fn fetch_quota() -> Result<QuotaResult, String> {
    let result = query_quota()?;

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

#[cfg(test)]
mod tests {
    use super::*;

    /// 内嵌样例：模拟 ~/.zcode/v2/config.json 顶层 provider map（不读本机真实文件）
    fn sample_providers() -> serde_json::Map<String, serde_json::Value> {
        let json = r#"{
            "builtin:bigmodel-coding-plan": {
                "name": "智谱 Coding Plan",
                "kind": "anthropic",
                "source": "builtin",
                "options": {
                    "apiKey": "cn-key-abcdef1234",
                    "baseURL": "https://open.bigmodel.cn/api/anthropic"
                }
            },
            "builtin:zai-coding-plan": {
                "name": "Z.ai Coding Plan",
                "kind": "anthropic",
                "source": "builtin",
                "options": {
                    "apiKey": "global-key-abcdef1234",
                    "baseURL": "https://api.z.ai/api/anthropic"
                }
            },
            "builtin:bigmodel-start-plan": {
                "name": "智谱轻量入门",
                "options": { "apiKey": "start-key-should-not-match" }
            },
            "custom:relay": {
                "name": "自定义中转",
                "options": { "apiKey": "relay-key" }
            }
        }"#;
        serde_json::from_str(json).unwrap()
    }

    /// a) 两个内置 Coding Plan 都有 key 时按固定顺序优先命中 bigmodel
    ///    （自动推断无端点配置，取确定性顺序）
    #[test]
    fn builtin_order_prefers_bigmodel() {
        let got = pick_coding_plan_api_key(&sample_providers()).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
        assert_eq!(got.1, "cn-key-abcdef1234");
        assert_eq!(got.2, "https://open.bigmodel.cn/api/anthropic");
    }

    /// b) 仅有 zai 凭证时命中 zai，并带出其 baseURL
    #[test]
    fn zai_only_picks_zai() {
        let json = r#"{
            "builtin:zai-coding-plan": {
                "options": {
                    "apiKey": "global-key-abcdef1234",
                    "baseURL": "https://api.z.ai/api/anthropic"
                }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        let got = pick_coding_plan_api_key(&providers).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
        assert_eq!(got.1, "global-key-abcdef1234");
        assert_eq!(got.2, "https://api.z.ai/api/anthropic");
    }

    /// c) 优先 key 的 apiKey 为空串时，回退到另一个含 coding-plan 的 provider
    #[test]
    fn empty_preferred_key_falls_back() {
        let json = r#"{
            "builtin:bigmodel-coding-plan": {
                "options": { "apiKey": "" }
            },
            "builtin:zai-coding-plan": {
                "options": { "apiKey": "global-key-abcdef1234" }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        let got = pick_coding_plan_api_key(&providers).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
        assert_eq!(got.1, "global-key-abcdef1234");
    }

    /// 优先 key 整个不存在时同样回退（仅有 zai 凭证）
    #[test]
    fn missing_preferred_key_falls_back() {
        let json = r#"{
            "builtin:zai-coding-plan": {
                "options": { "apiKey": "global-key-abcdef1234" }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        let got = pick_coding_plan_api_key(&providers).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
    }

    /// d) 只有 start-plan（及无关 provider）时返回 None
    #[test]
    fn start_plan_only_returns_none() {
        let json = r#"{
            "builtin:bigmodel-start-plan": {
                "options": { "apiKey": "start-key-should-not-match" }
            },
            "builtin:zai-start-plan": {
                "options": { "apiKey": "another-start-key" }
            },
            "custom:relay": {
                "options": { "apiKey": "relay-key" }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        assert!(pick_coding_plan_api_key(&providers).is_none());
    }

    /// e) provider map 为空返回 None
    #[test]
    fn empty_providers_returns_none() {
        let providers = serde_json::Map::new();
        assert!(pick_coding_plan_api_key(&providers).is_none());
    }

    /// apiKey 为纯空白时视同未配置（回退 / 不命中）
    #[test]
    fn whitespace_key_is_ignored() {
        let json = r#"{
            "builtin:bigmodel-coding-plan": {
                "options": { "apiKey": "   " }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        assert!(pick_coding_plan_api_key(&providers).is_none());
    }

    /// f) baseURL 缺失时三元组给空串（由 base_from_provider_url 兜底为国内站）
    #[test]
    fn missing_base_url_yields_empty() {
        let json = r#"{
            "builtin:bigmodel-coding-plan": {
                "options": { "apiKey": "cn-key-abcdef1234" }
            }
        }"#;
        let providers: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(json).unwrap();
        let got = pick_coding_plan_api_key(&providers).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
        assert_eq!(got.2, "");
    }

    /// 按 provider baseURL 推断额度接口 base：
    /// z.ai → api.z.ai；bigmodel / 空 → open.bigmodel.cn
    #[test]
    fn base_inference_from_provider_url() {
        assert_eq!(
            base_from_provider_url("https://api.z.ai/api/anthropic"),
            "https://api.z.ai"
        );
        assert_eq!(
            base_from_provider_url("https://open.bigmodel.cn/api/anthropic"),
            "https://open.bigmodel.cn"
        );
        assert_eq!(base_from_provider_url(""), "https://open.bigmodel.cn");
    }
}

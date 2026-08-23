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

/// ZCode v2 数据目录定位：默认 `~/.zcode/v2`；但 ZCode 支持「更改数据目录」
/// （记录在默认位置 setting.json 的 dataBaseDir 字段，如 `D:\app\ZCode-cache`），
/// 迁移后登录凭证与 config 全部写入 `{dataBaseDir}/.zcode/v2/`，默认位置只剩
/// 迁移前的旧数据——仍按默认位置读会永远拿到过期账号（捕获/额度查询均受影响）。
/// 迁移目录真实存在才启用；setting 缺失/损坏/字段为空一律回退默认位置。
pub(crate) fn zcode_v2_dir() -> Result<std::path::PathBuf, String> {
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    let default = home.join(".zcode").join("v2");
    let migrated = std::fs::read_to_string(default.join("setting.json"))
        .ok()
        .as_deref()
        .and_then(data_base_dir_from_setting)
        .map(|base| base.join(".zcode").join("v2"));
    match migrated {
        Some(dir) if dir.is_dir() => Ok(dir),
        _ => Ok(default),
    }
}

/// 从 setting.json 原文解析 dataBaseDir（缺失/空白/坏 JSON → None，纯函数便于单测）。
/// 相对路径不可用：按进程 CWD 判定 is_dir 不可控，直接回退默认位置。
fn data_base_dir_from_setting(raw: &str) -> Option<std::path::PathBuf> {
    let dir: String = serde_json::from_str::<serde_json::Value>(raw)
        .ok()?
        .get("dataBaseDir")?
        .as_str()?
        .trim()
        .to_string();
    if dir.is_empty() {
        return None;
    }
    let path = std::path::PathBuf::from(dir);
    path.is_absolute().then_some(path)
}

/// 读取 ZCode 数据目录 config.json 中登录 Coding Plan 后自动写入的凭证
/// （只读，绝不写回——该文件由 ZCode 客户端维护，外部写回极易把
/// ZCode 的登录态搞坏；key 的增删与刷新由 ZCode 客户端自行管理）。
/// 整个额度查询路径都不写 ZCode 数据目录；全应用唯一受控写该目录的位置
/// 是 accounts.rs 的切换事务（先退出 ZCode 再原文回写，详见其模块头注释）。
/// 返回 (provider_key, api_key, base_url)，其中 base_url 取该 provider
/// 的 options.baseURL（用于推断额度接口端点，缺失时为空串）。
///
/// 错误文案统一以「未找到 ZCode Coding Plan 凭证」开头：前端 QuotaPanel /
/// SummaryTab 以该固定前缀识别登录引导分支（后端改前缀须与前端同步）。
fn pick_from_config() -> Result<(String, String, String), String> {
    let path = zcode_v2_dir()
        .map_err(|e| {
            format!("未找到 ZCode Coding Plan 凭证：{e}，请先在 ZCode 客户端登录 Coding Plan 订阅")
        })?
        .join("config.json");
    if !path.exists() {
        return Err("未找到 ZCode Coding Plan 凭证（ZCode 数据目录下 config.json 不存在），请先在 ZCode 客户端登录 Coding Plan 订阅".into());
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
        "未找到 ZCode Coding Plan 凭证（config.json 中无可用 Coding Plan 凭证），请先在 ZCode 客户端登录 Coding Plan 订阅"
            .into(),
    )
}

/// 单个 provider 的凭证（纯解析，便于单测）：非空 apiKey（首尾空白去除）
/// + baseURL（缺失给空串，由 base_from_provider_url 兜底）。apiKey 无效返回 None。
pub(crate) fn provider_credential(v: &serde_json::Value) -> Option<(String, String)> {
    let api_key = v
        .get("options")
        .and_then(|o| o.get("apiKey"))
        .and_then(|k| k.as_str())
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())?;
    let base_url = v
        .get("options")
        .and_then(|o| o.get("baseURL"))
        .and_then(|u| u.as_str())
        .unwrap_or("")
        .to_string();
    Some((api_key, base_url))
}

/// 从 config.json 顶层 provider map 中选出 Coding Plan 凭证（纯解析，便于单测），
/// 返回 (provider_key, api_key, base_url)。
/// 优先按内置顺序取 builtin:bigmodel-coding-plan / builtin:zai-coding-plan；
/// 其 apiKey 为空或 key 不存在时，回退到任意 key 含 "coding-plan" 且 apiKey
/// 非空的 provider（用户通常只登录一个订阅，回退天然命中实际登录方）。
/// 注意：builtin:bigmodel-start-plan / builtin:zai-start-plan 是轻量入门订阅，
/// 不可用于查询订阅额度，好在它们的 key 不含 "coding-plan" 子串，天然被回退排除。
pub(crate) fn pick_coding_plan_api_key(
    providers: &serde_json::Map<String, serde_json::Value>,
) -> Option<(String, String, String)> {
    // 内置 Coding Plan provider 固定优先顺序（无端点配置后的确定性选择）
    for preferred in ["builtin:bigmodel-coding-plan", "builtin:zai-coding-plan"] {
        if let Some((api_key, base_url)) =
            providers.get(preferred).and_then(provider_credential)
        {
            return Some((preferred.to_string(), api_key, base_url));
        }
    }
    // 回退：任意 key 含 "coding-plan" 且 apiKey 非空（start-plan 不含该子串，被排除）
    providers
        .iter()
        .filter(|(k, _)| k.contains("coding-plan"))
        .find_map(|(k, v)| {
            provider_credential(v).map(|(key, base)| (k.clone(), key, base))
        })
}

/// 请求额度接口并解析（给定 apiKey 与 provider baseURL 的纯查询）。
///
/// 接口返回的 limits 中包含多个类型和窗口，不能只按 nextResetTime 排序：
/// 5 小时窗口刚刷新后可能没有 nextResetTime，反而会被排序到最后。
///
/// 注意：本函数不写 quota_history 快照。写采样由调用方决定：
/// fetch_quota（当前账号）与 account_quotas（全部账号）各自成功后经
/// snapshot_of 带账号指纹写入——读路径按指纹过滤，互不污染。
pub(crate) fn query_quota_with(token: &str, base_url: &str) -> Result<QuotaResult, String> {
    let base = base_from_provider_url(base_url);
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

    // 同一账号存在多个订阅时，limits 里可能出现多条同 (unit, number) 的窗口。
    // .find() 只按数组顺序取第一条，若服务端返回顺序在轮询间漂移，取到的
    // 订阅会来回切换，百分比跳变并污染"今日增量"的峰值-首条差值。
    // 因此匹配到多条时按 next_reset_time 升序取最近重置的一条（最活跃订阅），
    // None 排最后，保证轮询间选择稳定。
    let pick_stable = |pred: &dyn Fn(&QuotaLimit) -> bool| -> Option<QuotaLimit> {
        let mut matched: Vec<&QuotaLimit> = token_limits.iter().filter(|l| pred(l)).collect();
        matched.sort_by_key(|l| l.next_reset_time.unwrap_or(i64::MAX));
        matched.first().map(|l| (*l).clone())
    };

    let hour5 = pick_stable(&|l| l.unit == 3 && l.number == 5)
        // 兼容旧接口：刚刷新后的短窗口通常没有 nextResetTime。
        .or_else(|| {
            token_limits
                .iter()
                .find(|l| l.next_reset_time.is_none())
                .cloned()
        })
        .or_else(|| token_limits.first().cloned());

    let weekly = pick_stable(&|l| l.unit == 6 && l.number == 1).or_else(|| {
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

/// 请求额度接口并解析（凭证自动推断版）。
///
/// 凭证与接口端点均自动推断：读取 ZCode 客户端本地登录态选出的 provider，
/// 按其 options.baseURL 判断走 api.z.ai 还是 open.bigmodel.cn。
/// 对外行为与错误文案不变（pick_from_config 的错误前缀被前端识别为登录引导分支）。
pub fn query_quota() -> Result<QuotaResult, String> {
    let (_provider_key, token, base_url) = pick_from_config()?;
    query_quota_with(&token, &base_url)
}

/// 把一次成功的额度查询转为历史快照（fetch_quota 与 account_quotas 共用，
/// 避免两处字段映射漂移）。ts 由调用方取当下时间，account 为账号指纹。
pub(crate) fn snapshot_of(
    result: &QuotaResult,
    account: Option<&str>,
) -> crate::quota_history::QuotaSnapshot {
    crate::quota_history::QuotaSnapshot {
        ts: chrono::Local::now().timestamp_millis(),
        account: account.map(|s| s.to_string()),
        level: result.level.clone(),
        weekly_pct: result.weekly.as_ref().map(|w| w.percentage).unwrap_or(0),
        weekly_reset: result.weekly.as_ref().and_then(|w| w.next_reset_time),
        hour5_pct: result.hour5.as_ref().map(|h| h.percentage).unwrap_or(0),
        mcp_pct: result.mcp.as_ref().map(|m| m.percentage).unwrap_or(0),
        mcp_used: result.mcp.as_ref().and_then(|m| m.current_value),
        mcp_total: result.mcp.as_ref().and_then(|m| m.usage),
    }
}

/// 查询额度并写一条历史快照（供前端 fetch_quota 命令调用）。
/// 快照带当前登录账号指纹：quota_history 的账号敏感读路径（今日增量等）
/// 按指纹过滤，切换账号后互不污染。指纹解密失败时写 None（宁缺毋错，
/// 该条不参与任何账号的增量计算）。
pub fn fetch_quota() -> Result<QuotaResult, String> {
    // 指纹必须在查询前取：额度查询本身要读 config.json 选凭证并发起 HTTP
    // （最长 15s），若等查询结束才取指纹，切换事务可能已把 credentials.json
    // 换成新账号，出现"旧账号数值 + 新账号指纹"的错配采样。
    let account = crate::accounts::current_fingerprint().map(|fp| fp.user_id);

    let result = query_quota()?;

    // 复核指纹未变：查询期间发生账号切换则本条数值归属不明，丢弃采样
    // （最坏丢一条 30s 采样，30s 后补采），绝不写错配数据。
    if let Some(acc) = &account {
        if crate::accounts::current_fingerprint().map(|fp| fp.user_id).as_deref()
            != Some(acc.as_str())
        {
            return Ok(result);
        }
    }

    // 采样：每次成功查询追加一条快照（静默失败，不影响额度查询本身）。
    // 用本地时间作为采样 ts，与 model_usage.started_at (UTC) 保持同口径。
    let snap = snapshot_of(&result, account.as_deref());
    crate::quota_history::append_snapshot(&snap);

    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// setting.json 的 dataBaseDir 解析：正常路径/前后空白可解析，
    /// 空串、缺失、坏 JSON、非字符串均回退 None（对应回退默认目录）
    #[test]
    fn data_base_dir_from_setting_parses() {
        let got = data_base_dir_from_setting(r#"{"dataBaseDir":"D:\\app\\ZCode-cache"}"#);
        assert_eq!(
            got,
            Some(std::path::PathBuf::from("D:\\app\\ZCode-cache"))
        );
        // 前后空白保留语义（trim 后仍非空即可用）
        assert_eq!(
            data_base_dir_from_setting(r#"{ "dataBaseDir": "  D:\\data  " }"#),
            Some(std::path::PathBuf::from("D:\\data"))
        );
        // 空串 / 缺失 / 坏 JSON / 非字符串 / 相对路径 → None
        assert_eq!(data_base_dir_from_setting(r#"{"dataBaseDir":"   "}"#), None);
        assert_eq!(data_base_dir_from_setting(r#"{"other":1}"#), None);
        assert_eq!(data_base_dir_from_setting("not json"), None);
        assert_eq!(data_base_dir_from_setting(r#"{"dataBaseDir":123}"#), None);
        assert_eq!(
            data_base_dir_from_setting(r#"{"dataBaseDir":"ZCode-cache"}"#),
            None
        );
    }

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

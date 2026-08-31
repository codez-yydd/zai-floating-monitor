//! 通用 provider 配额查询分发骨架。
//!
//! 为凭证驱动的余额/订阅型 provider（moonshot / deepseek 及后续 11 个）提供
//! 统一的查询入口：读取该 provider 全部凭证 → 按 provider 分发到各自模块
//! （fetch_quota_entries）→ 每条凭证产出一条 ProviderQuotaEntry（与前端
//! types.ts 的 ProviderQuotaEntry/Window/Balance 字段一一对应，camelCase）。
//!
//! 工程纪律（对齐 kimi.rs / provider_credentials.rs 先例）：
//! - 网络：ureq 同步请求 + 15s 超时 + 复用 codex::resolve_proxy（环境变量 >
//!   系统代理 > 直连）；command 层 async + spawn_blocking 卸载到阻塞线程池；
//! - 并发：不持 PROVIDER_LOCK 做网络请求——先取凭证快照，网络查询，再回写
//!   last_check（record_check 内部自行短暂持锁）；
//! - 容错：单凭证查询失败产出 error/expired 条目，不阻塞其他凭证（凭证数少，
//!   串行循环即可，不引线程池）；
//! - 安全：错误消息用中文且不含 secret 片段；secret 只在 Rust 内部构造鉴权头。
//!
//! 后续接入新 provider 的步骤：新增 <provider>.rs 实现 fetch_quota_entries，
//! 在下方 match 加一个分支即可（未接入的 provider 返回空数组，前端显示
//! 「接入中」提示）。

use crate::provider_credentials;
use serde::Serialize;

// ============================================================
// 数据结构（与前端 types.ts 逐字段对齐；Option 字段缺省时省略，
// 序列化结果与前端 `?:` 可选字段语义一致）
// ============================================================

/// 单个用量窗口（如 5 小时窗 / 周窗 / 月窗）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaWindow {
    /// 窗口标识（provider 内部去重用，如 "hour5" / "weekly"）
    pub key: String,
    /// 展示标题（已本地化的短语，如 "5h" / "本周"）
    pub title: String,
    /// 已用百分比 0-100（缺省时前端只展示 used/total）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used_percent: Option<f64>,
    /// 已用量（配窗口 unit 展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub used: Option<f64>,
    /// 总量
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total: Option<f64>,
    /// 数量单位（"次" / "token" 等，已本地化）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    /// 下次重置时间（ms 时间戳）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resets_at: Option<i64>,
}

/// 按量计费余额（DeepSeek / Moonshot / 通义 Token 等充值型 provider）。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaBalance {
    /// 当前余额
    pub amount: f64,
    /// 币种符号或代码（"$" / "¥" / "CNY"）
    pub currency: String,
    /// 累计赠送（有值时前端与 topped_up 拆分展示）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub granted: Option<f64>,
    /// 累计充值
    #[serde(skip_serializing_if = "Option::is_none")]
    pub topped_up: Option<f64>,
}

/// 单条凭证的额度展示条目。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderQuotaEntry {
    /// 关联凭证 id（与 ProviderCredentialMeta.id 对应）
    pub credential_id: String,
    /// 凭证备注名（冗余存储，前端展示层无需回查凭证列表）
    pub label: String,
    /// "ok" | "expired" | "error" | "pending"
    pub status: String,
    pub windows: Vec<ProviderQuotaWindow>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub balance: Option<ProviderQuotaBalance>,
    /// 套餐名（"Pro" / "Max5" 等，前端展示为徽标）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plan_name: Option<String>,
    /// 查询失败 / 过期原因（中文，不含 secret）
    #[serde(skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
    /// 本次查询完成时刻（ms 时间戳）
    pub updated_at: i64,
}

// ============================================================
// 共用工具（各 provider 模块 + 后续接入复用）
// ============================================================

/// 单凭证单请求超时（秒）。
pub(crate) const QUOTA_TIMEOUT_SECS: u64 = 15;

/// 当前毫秒时间戳（各 provider 构造条目 updated_at 用，保持口径统一）。
pub(crate) fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 弹性数值解析（纯函数）：开放平台余额字段常见字符串数字（"110.00"）与
/// 数字双形态，统一在此收敛；空白/脏值返回 None（由调用方决定缺省语义）。
pub(crate) fn parse_flexible_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 双兼容取键（纯函数）：依次尝试多个键名（camelCase/snake_case 命名不
/// 统一的服务端，如 qoder 的 `totalQuota|total_quota`），返回第一个存在
/// 且非 null 的值；全缺返回 None。
pub(crate) fn get_any<'a>(
    v: &'a serde_json::Value,
    keys: &[&str],
) -> Option<&'a serde_json::Value> {
    keys.iter()
        .find_map(|k| v.get(*k))
        .filter(|v| !v.is_null())
}

/// 双兼容取数值：get_any + parse_flexible_f64 组合（字段名多形态 + 值
/// 字符串/数字双形态，两处弹性在此收敛）。
pub(crate) fn num_any(v: &serde_json::Value, keys: &[&str]) -> Option<f64> {
    get_any(v, keys).and_then(parse_flexible_f64)
}

/// 构建额度查询共用的 ureq Agent：指定超时 + 复用 codex::resolve_proxy
/// （环境变量 > 系统代理 > 直连；部分网络下直连不可达）。非关键请求
/// （如 grok settings 订阅名）可传更短超时。
pub(crate) fn quota_http_agent_timeout(secs: u64) -> ureq::Agent {
    let mut builder = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(secs));
    if let Some(url) = crate::codex::resolve_proxy() {
        match ureq::Proxy::new(&url) {
            Ok(p) => builder = builder.proxy(p),
            Err(e) => eprintln!("[zbar-quota] 代理地址无效（改为直连）: {url} ({e})"),
        }
    }
    builder.build()
}

/// 标准额度查询 Agent（15s 超时，绝大多数 provider 用）。
pub(crate) fn quota_http_agent() -> ureq::Agent {
    quota_http_agent_timeout(QUOTA_TIMEOUT_SECS)
}

/// 把 ureq 调用结果展平为 (HTTP 状态码, 响应体)。
/// ureq 2.x 对 4xx/5xx 走 `Error::Status(status, resp)` 而非 Ok 分支，这里
/// 统一展平，让各 provider 的解析纯函数按状态码分支（401/403 → expired 等）；
/// 网络层彻底失败（超时/DNS/连接被拒）返回 Err（中文原因，不含 URL 查询参数）。
pub(crate) fn flatten_response(
    result: Result<ureq::Response, ureq::Error>,
) -> Result<(u16, Option<String>), String> {
    match result {
        Ok(resp) => {
            let status = resp.status();
            let body = resp
                .into_string()
                .map_err(|e| format!("读取响应体失败: {e}"))?;
            Ok((status, Some(body)))
        }
        Err(ureq::Error::Status(status, resp)) => {
            // 4xx/5xx 也尽量带上响应体（部分平台把错误原因写在 body 里）
            let body = resp.into_string().ok();
            Ok((status, body))
        }
        Err(e) => Err(format!("网络错误或服务不可用: {e}")),
    }
}

// ============================================================
// 分发骨架（command 薄封装 + 单测入口）
// ============================================================

/// 查询某 provider 全部凭证的额度（串行逐凭证；每条凭证完成即回写
/// last_check——ok 回写 "ok"，expired/error 回写 "error" + 原因，供凭证卡
/// 显示最近校验结论）。无凭证 / provider 未接入返回空数组。
pub(crate) fn query_provider_quota(
    provider: &str,
) -> Result<Vec<ProviderQuotaEntry>, String> {
    // 0. 纯本地型 provider（OpenCode Go / Gemini CLI）：不走凭证快照与校验
    //    回写，直接读本地登录态/数据库。数据不存在返回空数组（tab 不出现）。
    if provider == "opencodego" {
        return Ok(crate::opencodego::fetch_quota_entries());
    }
    if provider == "gemini" {
        return Ok(crate::gemini::fetch_quota_entries());
    }
    // 1. 取凭证快照（文件读，锁内瞬时完成；网络请求不在锁内）
    let snapshots = provider_credentials::load_query_snapshots(provider)?;
    // grok 为混合型（本地 auth.json + 手动凭证），由其 fetch 内部合并两路，
    // 无手动凭证也可能有本地条目，不能按快照为空早返回；
    // 其余凭证型 provider 无凭证时早返回（行为不变）
    if provider != "grok" && snapshots.is_empty() {
        return Ok(vec![]);
    }
    // 2. 按 provider 分发（其余 provider 尚未接入：空数组，前端显示接入提示）
    let entries = match provider {
        "moonshot" => crate::moonshot::fetch_quota_entries(&snapshots),
        "deepseek" => crate::deepseek::fetch_quota_entries(&snapshots),
        "minimax" => crate::minimax::fetch_quota_entries(&snapshots),
        "grok" => crate::grok::fetch_quota_entries(&snapshots),
        // Claude 订阅手动凭证（kind=token，sk-ant-oat OAuth access token）：
        // 每条凭证调同一 OAuth usage 端点，解析复用 claude 模块函数。本地
        // 登录态不并入本链路——本地路径继续走 get_claude_usage 的
        // fetch_live_rate_limits（带 60s 缓存与历史采样），避免双查询；
        // 无手动凭证时上方空快照早返回已给出空 Vec。
        "claude" => crate::claude::fetch_manual_quota_entries(&snapshots),
        // Cursor 订阅手动凭证（kind=cookie，浏览器复制的 WorkosCursorSessionToken
        // Cookie 头；含旧 cursor.json cookie_header 的一次性迁移条目）：每条
        // 凭证调同一 usage-summary 端点堆叠展示。本地 auto 登录态不并入本链路
        //（主面板 get_cursor_usage 已展示，避免双查询）；无手动凭证时上方
        // 空快照早返回已给出空 Vec。
        "cursor" => crate::cursor::fetch_manual_quota_entries(&snapshots),
        // cookie 型：凭证 kind=cookie 的 secret 是浏览器 Cookie（或整段
        // cURL 粘贴），由 cookie_util 归一后做浏览器仿真请求
        "qoder" => crate::qoder::fetch_quota_entries(&snapshots),
        "longcat" => crate::longcat::fetch_quota_entries(&snapshots),
        "alibaba" => crate::alibaba::fetch_quota_entries(&snapshots),
        // cookie 型：百炼 Token 包（阿里 Token Plan），region 分国际/中国站，
        // Team 订阅摘要（积分池单窗口）+ Personal/Solo 滚动双窗口自动探测
        "alibabatoken" => crate::alibabatoken::fetch_quota_entries(&snapshots),
        // token 型：凭证 kind=token 的 secret 是浏览器复制的 Oasis-Token
        // （platform.stepfun.com 登录态 JWT），带 Oasis-Webid 绑定请求
        "stepfun" => crate::stepfun::fetch_quota_entries(&snapshots),
        // cookie 型：必需 api-platform_serviceToken + userId 两个 cookie
        "mimo" => crate::mimo::fetch_quota_entries(&snapshots),
        // Kimi 订阅凭证（kind=apiKey 直用 / kind=token 的 OAuth refresh_token
        // 换新，region 分大陆/国际站域名）。本地 CLI 登录态不并入本链路
        //（主面板 get_kimi_usage 已展示，避免双查询）；无凭证时上方空快照
        // 早返回已给出空 Vec。
        "kimi" => crate::kimi::fetch_quota_entries(&snapshots),
        _ => return Ok(vec![]),
    };
    // 3. 回写最近校验状态（record_check 内部短暂持锁做文件 IO；失败只记
    //    日志不阻塞返回——查询结果照常给前端，last_check 等下一轮补上）
    for entry in &entries {
        // 本地型条目（credential_id="local"）不对应凭证体系的任何条目，
        // 跳过回写（record_check 找不到会报错刷日志）
        if entry.credential_id == "local" {
            continue;
        }
        let (status, message) = if entry.status == "ok" {
            ("ok", None)
        } else {
            // expired 对凭证卡语义就是校验失败（Key 无效），归入 error 回写
            ("error", entry.message.as_deref())
        };
        if let Err(e) =
            provider_credentials::record_check(provider, &entry.credential_id, status, message)
        {
            eprintln!(
                "[zbar-quota] 回写 {provider} 凭证 {} 校验状态失败: {e}",
                entry.credential_id
            );
        }
    }
    Ok(entries)
}

/// 查询某 provider 全部凭证的额度（无凭证返回空数组）。
/// async + spawn_blocking：内部为同步 HTTP（ureq 串行查询，15s/凭证），
/// 必须卸载到阻塞线程池，避免同步 command 在主线程执行时网络慢冻结
/// 托盘/窗口事件（前端 120s 轮询 + 手动刷新）。
#[tauri::command]
pub async fn get_provider_quota(
    provider: String,
) -> Result<Vec<ProviderQuotaEntry>, String> {
    tauri::async_runtime::spawn_blocking(move || query_provider_quota(&provider))
        .await
        .map_err(|e| format!("额度查询任务失败: {e}"))?
}

// ============================================================
// 单元测试（纯函数部分）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flexible_number_parses_string_and_number() {
        // 字符串数字（开放平台余额的常见形态）
        assert_eq!(parse_flexible_f64(&serde_json::json!("110.00")), Some(110.0));
        assert_eq!(parse_flexible_f64(&serde_json::json!(" 0.5 ")), Some(0.5));
        // 数字原样
        assert_eq!(parse_flexible_f64(&serde_json::json!(42)), Some(42.0));
        assert_eq!(parse_flexible_f64(&serde_json::json!(1.25)), Some(1.25));
        // 脏值 / 空串 / 类型不符 → None
        assert_eq!(parse_flexible_f64(&serde_json::json!("abc")), None);
        assert_eq!(parse_flexible_f64(&serde_json::json!("")), None);
        assert_eq!(parse_flexible_f64(&serde_json::json!(null)), None);
        assert_eq!(parse_flexible_f64(&serde_json::json!(true)), None);
    }

    #[test]
    fn quota_entry_serializes_camel_case_and_skips_none() {
        let entry = ProviderQuotaEntry {
            credential_id: "abc-1".into(),
            label: "主账号".into(),
            status: "ok".into(),
            windows: vec![ProviderQuotaWindow {
                key: "hour5".into(),
                title: "5h".into(),
                used_percent: Some(30.0),
                used: None,
                total: None,
                unit: None,
                resets_at: Some(1_730_000_000_000),
            }],
            balance: Some(ProviderQuotaBalance {
                amount: 110.0,
                currency: "CNY".into(),
                granted: Some(10.0),
                topped_up: Some(100.0),
            }),
            plan_name: None,
            message: None,
            updated_at: 1_730_000_000_000,
        };
        let json = serde_json::to_value(&entry).unwrap();
        // camelCase 对齐前端 types.ts
        assert_eq!(json["credentialId"], "abc-1");
        assert_eq!(json["windows"][0]["usedPercent"], serde_json::json!(30.0));
        assert_eq!(json["windows"][0]["resetsAt"], serde_json::json!(1_730_000_000_000i64));
        assert_eq!(json["balance"]["toppedUp"], serde_json::json!(100.0));
        // Option::None 字段省略（与前端可选字段语义一致，不出 null）
        assert!(json.get("planName").is_none());
        assert!(json.get("message").is_none());
        assert!(json["windows"][0].get("used").is_none());
    }
}

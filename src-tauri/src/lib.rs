#![allow(linker_messages)]

mod agent_quota_history;
mod claude;
mod codex;
mod cursor;
mod db;
mod pricing;
mod quota;
mod quota_history;
mod shortcut;
mod sync;

use pricing::{load_pricing, save_pricing, ModelPrice, PricingConfig};
use quota::QuotaResult;
use chrono::TimeZone;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalPosition, Manager, PhysicalSize, WindowEvent,
};

/// 计费所需的字段抽象。ModelStat 与 BucketModelStat 都实现它，
/// 这样 cost_for 可同时服务 compute_cost 和 get_trend 等
/// （get_codex_usage / get_claude_usage 的桶内模型聚合同款）。
trait Billable {
    fn model_id(&self) -> &str;
    fn input_tokens(&self) -> i64;
    fn output_tokens(&self) -> i64;
    fn cache_read_tokens(&self) -> i64;
}

impl Billable for db::ModelStat {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

impl Billable for db::BucketModelStat {
    fn model_id(&self) -> &str {
        &self.model_id
    }
    fn input_tokens(&self) -> i64 {
        self.input_tokens
    }
    fn output_tokens(&self) -> i64 {
        self.output_tokens
    }
    fn cache_read_tokens(&self) -> i64 {
        self.cache_read_tokens
    }
}

/// 按 price map 计算单个模型的花费（每百万 token 计价）。
/// input_tokens 已包含 cache_read_tokens，缓存读部分按缓存价计费，
/// 剩余非缓存输入部分才按输入价计费。
/// 查找先精确匹配 db 原始形态；miss 时再按「小写 + 点号归一」兜底（价格条目
/// 与 db 的 model_id 大小写或点号/连字符写法可能不一致，让任何形态都能算出花费）。
/// 线性扫描仅在 miss 时发生。
fn cost_for<B: Billable>(s: &B, map: &BTreeMap<String, ModelPrice>) -> f64 {
    let lookup = |p: &ModelPrice| {
        let non_cache_input =
            (s.input_tokens() - s.cache_read_tokens()).max(0) as f64;
        (non_cache_input * p.input
            + s.output_tokens() as f64 * p.output
            + s.cache_read_tokens() as f64 * p.cache_read)
            / 1_000_000.0
    };
    if let Some(p) = map.get(s.model_id()) {
        return lookup(p);
    }
    let target = pricing::normalize_dots(&s.model_id().to_lowercase());
    map.iter()
        .find(|(k, _)| pricing::normalize_dots(&k.to_lowercase()) == target)
        .map(|(_, p)| lookup(p))
        .unwrap_or(0.0)
}

/// 当前 USD→CNY 汇率（与 Cursor 配置共用同一来源，每日自动更新；非法值回退 7.2）。
/// 价格只存美元，人民币花费 = 美元花费 × 该汇率实时折算。
fn load_fx_rate() -> f64 {
    let rate = cursor::load_cursor_config()
        .map(|c| c.usd_cny_rate)
        .unwrap_or(7.2);
    if rate > 0.0 {
        rate
    } else {
        7.2
    }
}

/// get_stats 命令的入参
#[derive(Debug, Deserialize)]
struct StatsRequest {
    from_ms: i64,
    to_ms: i64,
}

/// get_stats：返回时间范围内的统计 + 按模型分组。
/// async + spawn_blocking：SQLite 查询（busy_timeout 3s），前端每 30s 高频调用，
/// ZCode 写入高峰期同步执行会让主线程秒级阻塞，卸载到阻塞线程池。
#[tauri::command]
async fn get_stats(req: StatsRequest) -> Result<db::Stats, String> {
    tauri::async_runtime::spawn_blocking(move || db::query_stats(req.from_ms, req.to_ms))
        .await
        .map_err(|e| format!("统计查询任务失败: {e}"))?
}

/// list_models：列出所有出现过的模型，价格设置页的配价表单数据源。
/// 来源 = 本地 ZCode 库 ∪ Codex 导入库 ∪ Claude 导入库 ∪ 远端同步的全部设备模型
/// （让"其他设备在用、本机没有"的模型也能直接配价并参与价格更新检查）。
/// 按 (provider_id, model_id) 去重、按 model_id 排序。
/// 远端清单带 5 分钟缓存、失败静默降级为空；远端 HTTP 不能跑在主线程，
/// 与其他含网络/磁盘 I/O 的命令一样 async + spawn_blocking。
#[tauri::command]
async fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        // zcode 本地库是主数据源，失败照常报错；codex/claude/远端为增量来源，静默降级
        let zcode = db::list_models()?;
        let codex_models = codex::list_models().unwrap_or_default();
        let claude_models = claude::list_models().unwrap_or_default();
        let remote = sync::remote_models_cached()
            .into_iter()
            .map(|m| db::ModelInfo {
                // 远端记录 provider_id 可能为空，用来源标识兜底，保证去重键稳定
                provider_id: if m.provider_id.is_empty() {
                    m.source
                } else {
                    m.provider_id
                },
                model_id: m.model_id,
            })
            .collect();
        Ok(merge_model_lists(vec![zcode, codex_models, claude_models, remote]))
    })
    .await
    .map_err(|e| format!("模型列表查询任务失败: {e}"))?
}

/// 合并多个来源的模型清单：(provider_id, model_id) 去重、按 model_id 排序。
/// 纯函数（list_models 命令用），抽出来便于测试多来源去重与排序。
fn merge_model_lists(lists: Vec<Vec<db::ModelInfo>>) -> Vec<db::ModelInfo> {
    let mut seen = std::collections::HashSet::new();
    let mut all: Vec<db::ModelInfo> = Vec::new();
    for m in lists.into_iter().flatten() {
        if seen.insert((m.provider_id.clone(), m.model_id.clone())) {
            all.push(m);
        }
    }
    all.sort_by(|a, b| a.model_id.cmp(&b.model_id).then(a.provider_id.cmp(&b.provider_id)));
    all
}

/// get_pricing：读取价格配置
#[tauri::command]
fn get_pricing() -> Result<PricingConfig, String> {
    load_pricing()
}

/// set_pricing：保存价格配置
#[tauri::command]
fn set_pricing(config: PricingConfig) -> Result<(), String> {
    save_pricing(&config)
}

/// get_currency：读取货币偏好（"cny" | "usd"），供前端初始化
#[tauri::command]
fn get_currency() -> String {
    pricing::load_currency()
}

/// set_currency：保存货币偏好。前端切换货币时同步给后端，菜单栏标题据此显示。
/// 保存后立即刷新一次菜单栏标题，避免用户切换后还要等 30 秒后台周期。
/// 标题刷新含 SQLite 查询（开启同步后还有 HTTP 请求），必须卸载到阻塞线程池，
/// 否则同步 command 在主线程执行会阻塞 UI（与 spawn_title_updater 同款模式）。
#[tauri::command]
fn set_currency(currency: String, app: AppHandle) -> Result<(), String> {
    pricing::save_currency(&currency)?;
    let _ = tauri::async_runtime::spawn_blocking(move || {
        let title = today_tray_title(&app);
        let _ = app.tray_by_id("main").map(|t| t.set_title(Some(title)));
    });
    Ok(())
}

/// check_pricing_updates：对比用户当前配置与内置参考表（编译期嵌入），返回差异。
/// 仅用于"检查更新"提示，绝不自动覆盖。价格对比本身无网络请求（唯一例外：
/// 启用多设备同步时 remote_models_cached 缓存过期会顺带刷新一次设备模型清单）。
/// 差异判定只看 USD 原始价（人民币按汇率折算展示，由前端实时计算）。
/// 遍历主体 =「数据库实际调用过 ∪ 用户已手动配置 ∪ 远端同步（其他设备）」的模型：
/// 实际在用但两边都没价格的模型会以 missing 暴露（花费按 0 计）。
/// async + spawn_blocking：多库查询（SQLite/文件 IO）不能跑在主线程。
#[tauri::command]
async fn check_pricing_updates() -> Result<pricing::PricingDiff, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let user = load_pricing()?;
        // 相关模型 = 数据库里出现过的 + 用户已配置的
        let mut relevant: std::collections::HashSet<String> = std::collections::HashSet::new();
        db::list_models()?.into_iter().for_each(|m| {
            relevant.insert(m.model_id);
        });
        // Codex 导入库出现过的模型也纳入检查主体（未安装/导入失败时静默跳过，
        // 不影响 zcode 部分的检查）
        if let Ok(models) = codex::list_models() {
            models.into_iter().for_each(|m| {
                relevant.insert(m.model_id);
            });
        }
        // Claude 导入库同上
        if let Ok(models) = claude::list_models() {
            models.into_iter().for_each(|m| {
                relevant.insert(m.model_id);
            });
        }
        // 远端同步（其他设备上传）的模型同样纳入：本机没有但其他设备在用的
        // 模型（如 gpt-5.6-sol）也需要配价——内置表收录则提示新增可一键应用，
        // 未收录则以 missing 暴露提醒手动补价；服务器不可用时静默降级为空
        sync::remote_models_cached().into_iter().for_each(|m| {
            relevant.insert(m.model_id);
        });
        relevant.extend(user.usd.keys().cloned());

        Ok(pricing::diff_pricing(&user, &relevant))
    })
    .await
    .map_err(|e| format!("检查任务执行失败: {e}"))?
}

/// apply_pricing_updates：把用户勾选的价格项合并进 pricing 并保存。
/// items: Vec<{model_id, currency, price}>
#[derive(Debug, Deserialize)]
struct ApplyPriceItem {
    model_id: String,
    currency: String,
    price: pricing::ModelPrice,
}

#[tauri::command]
fn apply_pricing_updates(items: Vec<ApplyPriceItem>) -> Result<PricingConfig, String> {
    let tuples: Vec<(String, String, pricing::ModelPrice)> = items
        .into_iter()
        .map(|i| (i.model_id, i.currency, i.price))
        .collect();
    pricing::apply_updates(&tuples)
}

/// get_shortcut_config：读取全局快捷键配置
#[tauri::command]
fn get_shortcut_config() -> Result<shortcut::ShortcutConfig, String> {
    Ok(shortcut::load_shortcut())
}

/// set_shortcut_config：先验证再应用，成功后才持久化。
/// 顺序很重要：若先保存非法 accelerator，会导致后续启动永久注册失败。
/// apply 失败时（如 accelerator 非法/被占用），回滚到之前已保存的旧配置，
/// 保证运行时快捷键不会因一次失败的尝试而彻底失效。
#[tauri::command]
fn set_shortcut_config(
    config: shortcut::ShortcutConfig,
    app: AppHandle,
) -> Result<(), String> {
    // 记住旧配置，用于失败回滚
    let old = shortcut::load_shortcut();
    // 先应用（内部会先 unregister_all 再 register，注册失败返回 Err）
    if let Err(e) = apply_shortcut(&app, &config) {
        // 回滚：重新注册旧配置（旧配置此前已验证可用）
        let _ = apply_shortcut(&app, &old);
        return Err(e);
    }
    // 应用成功才保存，避免非法配置落盘
    shortcut::save_shortcut(&config)
}

/// 注销当前快捷键（设置关闭时用）。
#[tauri::command]
fn unregister_shortcut(app: AppHandle) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    gs.unregister_all().map_err(|e| format!("注销快捷键失败: {e}"))
}

/// fetch_quota：实时查询 Coding Plan 额度（5小时窗口 + 每周）。
/// 凭证与端点由后端自动推断（只读 ZCode 客户端登录态，无入参）。
/// async + spawn_blocking：内部为同步 HTTP（ureq），必须卸载到阻塞线程池，
/// 否则同步 command 在主线程执行时，网络慢会冻结托盘/窗口事件（前端每 30s 调一次）。
#[tauri::command]
async fn fetch_quota() -> Result<QuotaResult, String> {
    tauri::async_runtime::spawn_blocking(quota::fetch_quota)
        .await
        .map_err(|e| format!("额度查询任务失败: {e}"))?
}

// ===== 周额度追踪 / 对比页 =====
// 以下命令内部为全文件读取/逐行解析（quota_history，90 天约 38MB）或 SQLite 查询
// （busy_timeout 3s），对比页每 60s 触发一轮，统一 async + spawn_blocking
// 卸载到阻塞线程池，避免阻塞主线程（与 get_stats 同款模式）。

/// 读取全部额度快照历史（按 ts 升序）。
#[tauri::command]
async fn get_quota_history() -> Result<Vec<quota_history::QuotaSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(quota_history::load_all)
        .await
        .map_err(|e| format!("读取快照历史任务失败: {e}"))?
}

/// 解析快照为"智谱重置周期"列表（对比页用）。
#[tauri::command]
async fn get_weekly_compare() -> Result<Vec<quota_history::WeeklyPeriod>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let snaps = quota_history::load_all()?;
        Ok(quota_history::split_periods(&snaps))
    })
    .await
    .map_err(|e| format!("周期解析任务失败: {e}"))?
}

/// 用指定快照解析周额度周期。
/// 对比页在启用多设备筛选时会把本机与远端快照合并后传入，避免周期列表
/// 永远只由本机历史决定，导致“远端设备”没有周期或周期边界与 Token 不一致。
#[tauri::command]
async fn get_weekly_compare_for_snapshots(
    snapshots: Vec<quota_history::QuotaSnapshot>,
) -> Result<Vec<quota_history::WeeklyPeriod>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        Ok(quota_history::split_periods(&snapshots))
    })
    .await
    .map_err(|e| format!("指定快照周期解析任务失败: {e}"))?
}

/// 今日增量：(增量百分比, 今日采样数)。
#[tauri::command]
async fn get_today_delta() -> Result<(u32, u32), String> {
    tauri::async_runtime::spawn_blocking(quota_history::today_delta)
        .await
        .map_err(|e| format!("今日增量任务失败: {e}"))?
}

/// 读取 Codex / Claude / Cursor 的 Agent 额度快照历史。
#[derive(Debug, Deserialize)]
struct AgentQuotaHistoryRequest {
    from_ms: i64,
    to_ms: i64,
}

#[tauri::command]
async fn get_agent_quota_history(
    req: AgentQuotaHistoryRequest,
) -> Result<Vec<agent_quota_history::AgentQuotaSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        agent_quota_history::load_range(req.from_ms, req.to_ms)
    })
    .await
    .map_err(|e| format!("读取 Agent 额度快照任务失败: {e}"))?
}

/// 清空额度快照历史（设置页"清理历史"用）。
#[tauri::command]
fn clear_quota_history() -> Result<(), String> {
    quota_history::clear_history()?;
    agent_quota_history::clear_history()
}

/// 对比页"实际 token"列（本地部分）：对一组周期 [reset_at, end_at)
/// 逐周期聚合本地 model_usage 的 token。前端再合并远端。
#[tauri::command]
async fn get_compare_tokens(
    periods: Vec<(i64, i64)>,
) -> Result<Vec<WeeklyTokenBucket>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let buckets = db::query_period_buckets(&periods)?;
        Ok(buckets
            .into_iter()
            .map(|b| WeeklyTokenBucket {
                reset_at: b.reset_at,
                end_at: b.end_at,
                total_tokens: b.total_tokens,
                requests: b.requests,
            })
            .collect())
    })
    .await
    .map_err(|e| format!("对比 token 任务失败: {e}"))?
}

/// 按 Agent 和指定周期聚合 Token。
/// source: zai / codex / claude / cursor；周期区间统一使用 [reset_at, end_at)。
#[tauri::command]
async fn get_compare_tokens_for_agent(
    source: String,
    periods: Vec<(i64, i64)>,
) -> Result<Vec<WeeklyTokenBucket>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let buckets = match source.as_str() {
            "zai" => db::query_period_buckets(&periods),
            "codex" => codex::query_period_buckets(&periods),
            "claude" => claude::query_period_buckets(&periods),
            "cursor" => {
                let from_ms = periods.iter().map(|(from, _)| *from).min().unwrap_or(0);
                let to_ms = periods.iter().map(|(_, to)| *to).max().unwrap_or(0);
                cursor::fetch_cursor_period_buckets(from_ms, to_ms, &periods)
            }
            _ => Err(format!("未知的对比 Agent: {source}")),
        }?;
        Ok(buckets
            .into_iter()
            .map(|b| WeeklyTokenBucket {
                reset_at: b.reset_at,
                end_at: b.end_at,
                total_tokens: b.total_tokens,
                requests: b.requests,
            })
            .collect())
    })
    .await
    .map_err(|e| format!("按 Agent 对比 token 任务失败: {e}"))?
}

// ===== Cursor 用量统计 =====

/// get_cursor_usage 的入参
#[derive(Debug, Deserialize)]
struct CursorUsageRequest {
    from_ms: i64,
    to_ms: i64,
}

/// 拉取 Cursor 用量快照（套餐额度 + events 明细）。
/// async + spawn_blocking：内部为同步 HTTP（ureq），必须卸载到阻塞线程池，
/// 避免网络 I/O 冻结 Tauri 主线程（托盘、窗口事件循环）。
#[tauri::command]
async fn get_cursor_usage(req: CursorUsageRequest) -> Result<cursor::CursorSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let result = cursor::fetch_cursor_snapshot(req.from_ms, req.to_ms);
        if let Ok(snapshot) = &result {
            let has_today_quota = snapshot
                .today_quota
                .as_ref()
                .map(|quota| quota.auto_pct.is_some() || quota.api_pct.is_some())
                .unwrap_or(false);
            if has_today_quota {
                append_cursor_today_quota_snapshot(snapshot);
            } else {
                let mut windows = Vec::new();
                if let Some(plan) = &snapshot.plan {
                    if let Some(used_pct) = plan.auto_pct {
                        windows.push(agent_quota_history::AgentQuotaWindow {
                            key: "cursor_auto".into(),
                            used_pct,
                            reset_at: snapshot
                                .billing_cycle_end
                                .as_deref()
                                .and_then(parse_iso_ts_ms),
                        });
                    }
                    if let Some(used_pct) = plan.api_pct {
                        windows.push(agent_quota_history::AgentQuotaWindow {
                            key: "cursor_api".into(),
                            used_pct,
                            reset_at: snapshot
                                .billing_cycle_end
                                .as_deref()
                                .and_then(parse_iso_ts_ms),
                        });
                    }
                }
                append_agent_quota_snapshot("cursor", snapshot.membership_type.clone(), windows);
            }
        }
        result
    })
    .await
    .map_err(|e| format!("Cursor 后台任务失败: {e}"))?
}

/// 读取 Cursor 配置
#[tauri::command]
fn get_cursor_config() -> Result<cursor::CursorConfig, String> {
    cursor::load_cursor_config()
}

/// 保存 Cursor 配置
#[tauri::command]
fn set_cursor_config(config: cursor::CursorConfig) -> Result<(), String> {
    cursor::save_cursor_config(&config)
}

/// fetch_fx_rate：立即联网获取最新 USD→CNY 汇率（多源容错）并写入 cursor 配置，
/// 返回 (汇率, 来源名)。设置页「立即更新」按钮用。
/// async + spawn_blocking：内部为同步 HTTP（ureq），必须卸载到阻塞线程池，
/// 避免网络 I/O 冻结 Tauri 主线程（与 check_pricing_updates 同款模式）。
#[tauri::command]
async fn fetch_fx_rate() -> Result<(f64, String), String> {
    tauri::async_runtime::spawn_blocking(cursor::fetch_fx_rate)
        .await
        .map_err(|e| format!("汇率获取任务失败: {e}"))?
}

// ===== Codex 用量统计 =====

/// get_codex_usage 的入参：时间范围 + 分桶粒度（"hour" | "day"）
#[derive(Debug, Deserialize)]
struct CodexUsageRequest {
    from_ms: i64,
    to_ms: i64,
    bucket: String,
}

/// get_codex_usage 返回的 Codex 快照：
/// 本地导入库统计 + 趋势（含花费，与 get_trend 同款计算）+ 最新订阅额度。
#[derive(Debug, Serialize)]
struct CodexSnapshot {
    stats: db::Stats,
    trend: Vec<TrendBucket>,
    rate_limits: Option<codex::CodexRateLimits>,
}

/// 拉取 Codex 用量快照（本地 sessions jsonl 增量导入 + 聚合查询）。
/// async + spawn_blocking：首次导入要解析大量会话文件（文件 IO + SQLite 写入），
/// 与 get_stats 同款卸载到阻塞线程池，避免阻塞主线程。
#[tauri::command]
async fn get_codex_usage(req: CodexUsageRequest) -> Result<CodexSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let stats = codex::query_stats(req.from_ms, req.to_ms)?;
        if let Ok(added) = codex::backfill_today_rate_limit_history() {
            if added > 0 {
                eprintln!("[zbar-codex] 已补齐今日 {added} 条额度快照");
            }
        }
        let buckets = codex::query_trend(req.from_ms, req.to_ms, &req.bucket)?;
        let pricing = load_pricing().unwrap_or_default();

        // 额度：优先实时接口（wham/usage，参照 CodexBar，60s 缓存），
        // 失败（未登录/网络不通/接口变更）静默降级到本地快照（已滤过期窗口）
        let live_rate_limits = codex::fetch_live_rate_limits_with_freshness();
        let rate_limits = match &live_rate_limits {
            Ok((live, _)) => live
                .clone()
                .or_else(|| codex::latest_rate_limits().ok().flatten()),
            Err(_) => codex::latest_rate_limits().ok().flatten(),
        };
        // 只有实时接口成功返回的值才写入历史；本地 rate_limits_state 是旧快照，
        // 网络失败时不能重复采样，避免把陈旧数据伪装成今日用量。
        if let Ok((Some(live), true)) = live_rate_limits {
            let mut windows = Vec::new();
            if let Some(used_pct) = live.primary_pct {
                windows.push(agent_quota_history::AgentQuotaWindow {
                    key: "hour5".into(),
                    used_pct,
                    reset_at: live.primary_reset_at,
                });
            }
            if let Some(used_pct) = live.secondary_pct {
                windows.push(agent_quota_history::AgentQuotaWindow {
                    key: "weekly".into(),
                    used_pct,
                    reset_at: live.secondary_reset_at,
                });
            }
            append_agent_quota_snapshot("codex", live.plan_type.clone(), windows);
        }

        // 花费计算与 get_trend 完全同款：桶内按模型聚合后用 cost_for 求和。
        // 只存美元价：人民币花费 = 美元花费 × 当前汇率（实时折算）
        let fx = load_fx_rate();
        let trend = buckets
            .into_iter()
            .map(|b| {
                let cost_usd = b
                    .by_model
                    .iter()
                    .map(|m| cost_for(m, &pricing.usd))
                    .sum::<f64>();
                TrendBucket {
                    label: b.label,
                    total_tokens: b.total_tokens,
                    requests: b.requests,
                    cost_cny: cost_usd * fx,
                    cost_usd,
                }
            })
            .collect();

        Ok(CodexSnapshot {
            stats,
            trend,
            rate_limits,
        })
    })
    .await
    .map_err(|e| format!("Codex 查询任务失败: {e}"))?
}

/// 诊断 Codex 数据导入（排查"暂无数据"问题）
#[tauri::command]
async fn get_codex_debug() -> Result<codex::CodexDebugInfo, String> {
    tauri::async_runtime::spawn_blocking(codex::debug_info)
        .await
        .map_err(|e| format!("Codex 诊断任务失败: {e}"))?
}

// ===== Claude 用量统计 =====

/// get_claude_usage 的入参：时间范围 + 分桶粒度（"hour" | "day"）
#[derive(Debug, Deserialize)]
struct ClaudeUsageRequest {
    from_ms: i64,
    to_ms: i64,
    bucket: String,
}

/// get_claude_usage 返回的 Claude 快照：
/// 本地导入库统计 + 趋势（含花费，与 get_trend 同款计算）+ 实时订阅额度。
#[derive(Debug, Serialize)]
struct ClaudeSnapshot {
    stats: db::Stats,
    trend: Vec<TrendBucket>,
    rate_limits: Option<claude::ClaudeRateLimits>,
}

/// 拉取 Claude 用量快照（本地 projects jsonl 增量导入 + 聚合查询）。
/// async + spawn_blocking：与 get_codex_usage 同款卸载到阻塞线程池。
/// 额度只有实时来源（会话文件无 rate_limits，与 Codex 不同）：OAuth 端点
/// 失败（未登录订阅/网络不通/第三方中转）静默降级为 null，额度块不展示。
#[tauri::command]
async fn get_claude_usage(req: ClaudeUsageRequest) -> Result<ClaudeSnapshot, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let stats = claude::query_stats(req.from_ms, req.to_ms)?;
        let buckets = claude::query_trend(req.from_ms, req.to_ms, &req.bucket)?;
        let pricing = load_pricing().unwrap_or_default();

        let live_rate_limits = claude::fetch_live_rate_limits_with_freshness();
        let rate_limits = live_rate_limits.clone().ok().and_then(|(live, _)| live);
        if let Ok((Some(live), true)) = live_rate_limits {
            let mut windows = Vec::new();
            if let Some(used_pct) = live.primary_pct {
                windows.push(agent_quota_history::AgentQuotaWindow {
                    key: "hour5".into(),
                    used_pct,
                    reset_at: live.primary_reset_at,
                });
            }
            if let Some(used_pct) = live.secondary_pct {
                windows.push(agent_quota_history::AgentQuotaWindow {
                    key: "weekly".into(),
                    used_pct,
                    reset_at: live.secondary_reset_at,
                });
            }
            append_agent_quota_snapshot("claude", live.plan_type.clone(), windows);
        }

        // 花费计算与 get_trend 完全同款：桶内按模型聚合后用 cost_for 求和。
        // 只存美元价：人民币花费 = 美元花费 × 当前汇率（实时折算）
        let fx = load_fx_rate();
        let trend = buckets
            .into_iter()
            .map(|b| {
                let cost_usd = b
                    .by_model
                    .iter()
                    .map(|m| cost_for(m, &pricing.usd))
                    .sum::<f64>();
                TrendBucket {
                    label: b.label,
                    total_tokens: b.total_tokens,
                    requests: b.requests,
                    cost_cny: cost_usd * fx,
                    cost_usd,
                }
            })
            .collect();

        Ok(ClaudeSnapshot {
            stats,
            trend,
            rate_limits,
        })
    })
    .await
    .map_err(|e| format!("Claude 查询任务失败: {e}"))?
}

/// 诊断 Claude 数据导入（排查"暂无数据"问题）
#[tauri::command]
async fn get_claude_debug() -> Result<claude::ClaudeDebugInfo, String> {
    tauri::async_runtime::spawn_blocking(claude::debug_info)
        .await
        .map_err(|e| format!("Claude 诊断任务失败: {e}"))?
}

fn append_agent_quota_snapshot(
    source: &str,
    plan_type: Option<String>,
    windows: Vec<agent_quota_history::AgentQuotaWindow>,
) {
    append_agent_quota_snapshot_at(
        source,
        plan_type,
        chrono::Local::now().timestamp_millis(),
        windows,
    );
}

fn append_agent_quota_snapshot_at(
    source: &str,
    plan_type: Option<String>,
    ts: i64,
    windows: Vec<agent_quota_history::AgentQuotaWindow>,
) {
    agent_quota_history::append_snapshot(&agent_quota_history::AgentQuotaSnapshot {
        source: source.to_string(),
        ts,
        plan_type,
        windows,
    });
}

/// 将 Cursor events 的今日扣费换算成 Auto / API 的今日百分比增量。
/// 旧历史里保存的是 provider 当前周期百分比，因此首次切换到 events 口径时
/// 先写入一个当天基线，再写入“基线 + 今日扣费”，避免丢掉已有采样或重复累计。
fn append_cursor_today_quota_snapshot(snapshot: &cursor::CursorSnapshot) {
    let Some(today) = snapshot.today_quota.as_ref() else {
        return;
    };
    let Some(plan) = snapshot.plan.as_ref() else {
        return;
    };
    let now = chrono::Local::now().timestamp_millis();
    let day_start = chrono::Local::now()
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| chrono::Local.from_local_datetime(&naive).single())
        .map(|value| value.timestamp_millis())
        .unwrap_or(now);
    let reset_at = snapshot
        .billing_cycle_end
        .as_deref()
        .and_then(parse_iso_ts_ms);
    let existing = agent_quota_history::load_range(day_start, now.saturating_add(1))
        .unwrap_or_default();

    let mut baseline_windows = Vec::new();
    let mut current_windows = Vec::new();
    for (key, daily_pct, current_pct) in [
        ("cursor_auto", today.auto_pct, plan.auto_pct),
        ("cursor_api", today.api_pct, plan.api_pct),
    ] {
        let Some(daily_pct) = daily_pct.filter(|pct| pct.is_finite() && *pct > 0.0) else {
            continue;
        };
        let baseline = existing
            .iter()
            .filter(|snapshot| snapshot.source == "cursor")
            .flat_map(|item| {
                item.windows
                    .iter()
                    .filter(move |window| window.key == key && window.reset_at == reset_at)
                    .map(|window| window.used_pct)
            })
            .filter(|pct| pct.is_finite())
            .min_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal))
            .unwrap_or_else(|| (current_pct.unwrap_or(daily_pct) - daily_pct).max(0.0));
        baseline_windows.push(agent_quota_history::AgentQuotaWindow {
            key: key.to_string(),
            used_pct: baseline,
            reset_at,
        });
        current_windows.push(agent_quota_history::AgentQuotaWindow {
            key: key.to_string(),
            used_pct: (baseline + daily_pct).clamp(0.0, 100.0),
            reset_at,
        });
    }

    if current_windows.is_empty() {
        return;
    }

    let has_baseline = existing.iter().any(|item| {
        item.source == "cursor"
            && item.ts / 1000 == day_start / 1000
            && item.windows.iter().any(|window| {
                baseline_windows.iter().any(|baseline| {
                    baseline.key == window.key && baseline.reset_at == window.reset_at
                })
            })
    });
    if !has_baseline {
        append_agent_quota_snapshot_at(
            "cursor",
            snapshot.membership_type.clone(),
            day_start,
            baseline_windows,
        );
    }
    append_agent_quota_snapshot_at(
        "cursor",
        snapshot.membership_type.clone(),
        now,
        current_windows,
    );
}

fn parse_iso_ts_ms(value: &str) -> Option<i64> {
    chrono::DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|dt| dt.timestamp_millis())
}

/// 对比页：单个周期的 token 聚合结果。
#[derive(Debug, Serialize)]
struct WeeklyTokenBucket {
    /// 周期开始（重置时间），作为 label 匹配键
    reset_at: i64,
    /// 周期结束时间
    end_at: i64,
    /// 桶内总 token
    total_tokens: i64,
    /// 桶内总请求数
    requests: i64,
}

/// compute_cost：根据统计 + 价格，计算花费（前端也会自己算，这里提供一份供托盘文字用）
#[derive(Debug, Serialize)]
struct CostResult {
    total_cny: f64,
    total_usd: f64,
    per_model_cny: Vec<ModelCost>,
    per_model_usd: Vec<ModelCost>,
}

#[derive(Debug, Serialize)]
struct ModelCost {
    model_id: String,
    cost: f64,
}

/// async + spawn_blocking：内部为 SQLite 查询（busy_timeout 3s），前端每 30s
/// 高频调用，与 get_stats 同款卸载到阻塞线程池，避免写入高峰期阻塞主线程。
#[tauri::command]
async fn compute_cost(req: StatsRequest) -> Result<CostResult, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let stats = db::query_stats(req.from_ms, req.to_ms)?;
        let pricing = load_pricing().unwrap_or_default();
        // 只存美元价：人民币花费 = 美元花费 × 当前汇率（实时折算）
        let fx = load_fx_rate();

        let per_model_usd: Vec<ModelCost> = stats
            .by_model
            .iter()
            .map(|s| ModelCost {
                model_id: s.model_id.clone(),
                cost: cost_for(s, &pricing.usd),
            })
            .collect();
        let per_model_cny: Vec<ModelCost> = per_model_usd
            .iter()
            .map(|m| ModelCost {
                model_id: m.model_id.clone(),
                cost: m.cost * fx,
            })
            .collect();

        Ok(CostResult {
            total_cny: per_model_cny.iter().map(|m| m.cost).sum(),
            total_usd: per_model_usd.iter().map(|m| m.cost).sum(),
            per_model_cny,
            per_model_usd,
        })
    })
    .await
    .map_err(|e| format!("花费计算任务失败: {e}"))?
}

/// 把报告内容写入 ~/.zbar/reports/<filename>，并在系统文件管理器中打开该目录。
/// content: Markdown 文本；filename: 如 "周报-2026-08-05.md"
#[tauri::command]
fn save_report(content: String, filename: String) -> Result<String, String> {
    let dir = pricing::config_dir()?.join("reports");
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建报告目录失败: {e}"))?;
    let path = dir.join(&filename);
    std::fs::write(&path, content).map_err(|e| format!("写入报告失败: {e}"))?;
    // 打开所在目录（而非文件本身），便于用户查看
    open::that(&dir).map_err(|e| format!("打开目录失败: {e}"))?;
    Ok(path.display().to_string())
}

// ===== 窗口置顶常驻（仅 Windows）=====

/// 置顶状态配置文件路径：~/.zbar/pin.json
fn pin_config_path() -> Result<std::path::PathBuf, String> {
    Ok(pricing::config_dir()?.join("pin.json"))
}

/// 读取置顶状态。文件不存在或解析失败时默认返回 false（不置顶），
/// 保证首次运行与异常情况下面板行为与原版一致。
fn load_pin() -> Result<bool, String> {
    let path = pin_config_path()?;
    if !path.exists() {
        return Ok(false);
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取置顶配置失败: {e}"))?;
    // 用 serde_json 解析 { "enabled": true }，兼容缺省字段
    let v: serde_json::Value =
        serde_json::from_str(&data).map_err(|e| format!("解析置顶配置失败: {e}"))?;
    Ok(v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false))
}

/// 写入置顶状态到 pin.json，持久化以便重启后恢复常驻。
fn save_pin(enabled: bool) -> Result<(), String> {
    let dir = pricing::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = pin_config_path()?;
    let data = serde_json::json!({ "enabled": enabled }).to_string();
    std::fs::write(&path, data).map_err(|e| format!("写入置顶配置失败: {e}"))
}

/// get_pin：读取当前窗口置顶状态
#[tauri::command]
fn get_pin() -> Result<bool, String> {
    load_pin()
}

/// set_pin：保存置顶状态并立即应用到 panel 窗口。
/// - enabled=true：保持 always_on_top + 立即显示，确保用户切换后立刻可见常驻
/// - enabled=false：仅取消 always_on_top（面板本就配 alwaysOnTop，此处恢复默认），
///   不主动隐藏，让随后的失焦事件按原逻辑隐藏
#[tauri::command]
fn set_pin(enabled: bool, app: AppHandle) -> Result<(), String> {
    save_pin(enabled)?;
    if let Some(window) = app.get_webview_window("panel") {
        let _ = window.set_always_on_top(enabled);
        if enabled {
            // 开启常驻：立即显示并聚焦，让用户切换后即刻可见
            let _ = window.show();
            let _ = window.set_focus();
        }
    }
    Ok(())
}

// ===== 多设备同步命令 =====

use sync::{
    AutoCleanupServerRequest, CleanupServerRequest, CleanupStatus, DeviceInfo,
    MergeDevicesRequest, RemoteAgentQuotaSnapshot, RemoteAgentQuotaSnapshotRequest,
    RemoteSnapshot, RemoteSnapshotRequest, RemoteUsage, RemoteUsageRequest,
    RenameDeviceRequest, SyncConfig, SyncOutcome,
};

/// 读取同步配置
#[tauri::command]
fn get_sync_config() -> Result<SyncConfig, String> {
    sync::load_sync_config()
}

/// 保存同步配置（仅持久化，不触发网络请求）
#[tauri::command]
fn set_sync_config(config: SyncConfig) -> Result<(), String> {
    sync::save_sync_config(&config)
}

// 以下 sync 系列命令内部直通 sync.rs 的 ureq 同步 HTTP（超时 10-15s），
// 同步 command 在 Tauri v2 跑主线程，网络慢会冻结托盘/窗口事件。
// 统一 async + spawn_blocking 卸载到阻塞线程池（与 fetch_quota 同款模式）。

/// 向服务器注册设备（UI 填写 server_url + master_token + name 后调用）
#[tauri::command]
async fn register_device(req: sync::RegisterRequest) -> Result<SyncConfig, String> {
    tauri::async_runtime::spawn_blocking(move || sync::register_device(req))
        .await
        .map_err(|e| format!("注册设备任务失败: {e}"))?
}

/// 手动触发一次增量上传
#[tauri::command]
async fn sync_now() -> Result<SyncOutcome, String> {
    tauri::async_runtime::spawn_blocking(sync::upload_incremental)
        .await
        .map_err(|e| format!("同步任务失败: {e}"))?
}

/// 断开连接（清凭证，不删服务器数据）
#[tauri::command]
async fn disconnect_device() -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(sync::disconnect)
        .await
        .map_err(|e| format!("断开连接任务失败: {e}"))?
}

/// 拉取远端聚合数据（其他设备）。
/// 在前端 30s 后台刷新链路上（每轮 × 4 个范围），必须卸载主线程。
#[tauri::command]
async fn remote_usage(req: RemoteUsageRequest) -> Result<RemoteUsage, String> {
    tauri::async_runtime::spawn_blocking(move || sync::fetch_remote_usage(req))
        .await
        .map_err(|e| format!("远端用量任务失败: {e}"))?
}

/// 拉取远端额度快照（带 device_id，供对比页/报告页跨设备周额度解析）
#[tauri::command]
async fn remote_snapshots(req: RemoteSnapshotRequest) -> Result<Vec<RemoteSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(move || sync::fetch_remote_snapshots(req))
        .await
        .map_err(|e| format!("远端快照任务失败: {e}"))?
}

/// 拉取远端 Agent 额度快照，供今日增量计算按设备筛选。
#[tauri::command]
async fn remote_agent_quota_snapshots(
    req: RemoteAgentQuotaSnapshotRequest,
) -> Result<Vec<RemoteAgentQuotaSnapshot>, String> {
    tauri::async_runtime::spawn_blocking(move || sync::fetch_remote_agent_quota_snapshots(req))
        .await
        .map_err(|e| format!("远端 Agent 额度快照任务失败: {e}"))?
}

/// 拉取设备列表
#[tauri::command]
async fn list_remote_devices() -> Result<Vec<DeviceInfo>, String> {
    tauri::async_runtime::spawn_blocking(sync::fetch_devices)
        .await
        .map_err(|e| format!("设备列表任务失败: {e}"))?
}

/// 查询清理状态
#[tauri::command]
async fn get_cleanup_status() -> Result<CleanupStatus, String> {
    tauri::async_runtime::spawn_blocking(sync::fetch_cleanup_status)
        .await
        .map_err(|e| format!("清理状态任务失败: {e}"))?
}

/// 执行服务端清理
#[tauri::command]
async fn cleanup_server(req: CleanupServerRequest) -> Result<sync::CleanupResult, String> {
    tauri::async_runtime::spawn_blocking(move || sync::cleanup_server(req))
        .await
        .map_err(|e| format!("服务端清理任务失败: {e}"))?
}

/// 合并设备：把来源设备数据并入目标设备后删除来源
#[tauri::command]
async fn merge_devices(req: MergeDevicesRequest) -> Result<sync::MergeResult, String> {
    tauri::async_runtime::spawn_blocking(move || sync::merge_devices(req))
        .await
        .map_err(|e| format!("合并设备任务失败: {e}"))?
}

/// 修改设备显示名
#[tauri::command]
async fn rename_device(req: RenameDeviceRequest) -> Result<sync::RenameResult, String> {
    tauri::async_runtime::spawn_blocking(move || sync::rename_device(req))
        .await
        .map_err(|e| format!("重命名设备任务失败: {e}"))?
}

/// 配置服务端自动清理
#[tauri::command]
async fn set_auto_cleanup(req: AutoCleanupServerRequest) -> Result<sync::AutoCleanupConfig, String> {
    tauri::async_runtime::spawn_blocking(move || sync::set_auto_cleanup(req))
        .await
        .map_err(|e| format!("自动清理配置任务失败: {e}"))?
}

/// 查询本机待上传的记录数（zcode 与 codex/claude 两个派生库的
/// 「max_rowid - 游标」之和，各取 max(0)），供同步面板显示。
/// codex/claude 查询失败按 0 计（未安装时不应影响其他来源显示）。
/// async + spawn_blocking：派生库首次导入可能解析大量会话文件，不能卡主线程。
#[tauri::command]
async fn pending_upload_count() -> Result<i64, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let cfg = sync::load_sync_config().unwrap_or_default();
        let zcode = (db::max_rowid()? - cfg.last_uploaded_rowid).max(0);
        let codex = codex::max_rowid()
            .map(|m| (m - cfg.last_uploaded_codex_rowid).max(0))
            .unwrap_or(0);
        let claude = claude::max_rowid()
            .map(|m| (m - cfg.last_uploaded_claude_rowid).max(0))
            .unwrap_or(0);
        Ok(zcode + codex + claude)
    })
    .await
    .map_err(|e| format!("待上传统计任务失败: {e}"))?
}

/// get_trend 的入参：时间范围 + 分桶粒度
#[derive(Debug, Deserialize)]
struct TrendRequest {
    from_ms: i64,
    to_ms: i64,
    /// "hour" | "day"
    bucket: String,
}

/// 趋势图用：单个桶的汇总（含两种货币花费，前端无需再算）
#[derive(Debug, Serialize)]
struct TrendBucket {
    /// 桶标签："14:00"（小时）或 "08-04"（日）
    label: String,
    /// 桶内总 token
    total_tokens: i64,
    /// 桶内总请求数
    requests: i64,
    /// 桶内人民币花费
    cost_cny: f64,
    /// 桶内美元花费
    cost_usd: f64,
}

/// get_trend：返回时间范围内的分桶统计，供趋势图使用。
/// 粒度由 bucket 决定（hour/day），桶数随范围自适应。
/// async + spawn_blocking：内部为 SQLite 查询（busy_timeout 3s），前端每 30s
/// 高频调用，与 get_stats 同款卸载到阻塞线程池，避免写入高峰期阻塞主线程。
#[tauri::command]
async fn get_trend(req: TrendRequest) -> Result<Vec<TrendBucket>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        let buckets = db::query_trend(req.from_ms, req.to_ms, &req.bucket)?;
        let pricing = load_pricing().unwrap_or_default();
        // 只存美元价：人民币花费 = 美元花费 × 当前汇率（实时折算）
        let fx = load_fx_rate();

        let out = buckets
            .into_iter()
            .map(|b| {
                let cost_usd = b
                    .by_model
                    .iter()
                    .map(|m| cost_for(m, &pricing.usd))
                    .sum::<f64>();
                TrendBucket {
                    label: b.label,
                    total_tokens: b.total_tokens,
                    requests: b.requests,
                    cost_cny: cost_usd * fx,
                    cost_usd,
                }
            })
            .collect();

        Ok(out)
    })
    .await
    .map_err(|e| format!("趋势查询任务失败: {e}"))?
}

/// 调整原生毛玻璃视图的不透明度，让背景保持柔和透出而不让文字穿透。
/// Tauri 会把 NSVisualEffectView 放在 contentView 下方，并用这个 tag 标记它。
#[cfg(target_os = "macos")]
fn tune_panel_vibrancy(window: &tauri::WebviewWindow) {
    use objc2_app_kit::{NSColor, NSWindow};

    const BLUR_VIEW_TAG: isize = 91_376_254;

    let Ok(ns_window_ptr) = window.ns_window() else {
        return;
    };

    // SAFETY: Tauri 返回的是当前 macOS NSWindow 的有效指针，且 setup 在主线程执行。
    unsafe {
        let ns_window: &NSWindow = &*ns_window_ptr.cast();
        ns_window.setOpaque(false);
        let clear = NSColor::clearColor();
        ns_window.setBackgroundColor(Some(&clear));

        if let Some(content_view) = ns_window.contentView() {
            for view in content_view.subviews().iter() {
                if view.tag() == BLUR_VIEW_TAG {
                    // popover 材质需要保持较高 alpha，才能把后面的文字模糊成
                    // 柔和的背景，而不是直接叠在内容文字上。
                    view.setAlphaValue(0.90);
                    break;
                }
            }
        }
    }
}

/// 切换面板窗口显示/隐藏，并定位到托盘附近（紧贴菜单栏/任务栏）。
/// click_pos: 点击位置（逻辑像素 x, y），来自托盘点击事件。
/// 坐标系：左上角原点，x 向右增，y 向下增。
fn toggle_panel(app: &AppHandle, click_pos: Option<(f64, f64)>) {
    let Some(window) = app.get_webview_window("panel") else {
        return;
    };
    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    let scale = window.scale_factor().unwrap_or(1.0);
    let win_size = window.outer_size().unwrap_or(PhysicalSize {
        width: 360,
        height: 560,
    });
    let win_w = win_size.width as f64 / scale;
    #[cfg(not(target_os = "macos"))]
    let win_h = win_size.height as f64 / scale;

    if let (Some(mon), Some((cx, _cy))) =
        (window.current_monitor().ok().flatten(), click_pos)
    {
        let mon_w = mon.size().width as f64 / scale;
        #[cfg(not(target_os = "macos"))]
        let mon_h = mon.size().height as f64 / scale;

        // 水平：面板右边对齐点击位置（图标在菜单栏右端，面板向左展开），不溢出
        let mut x = cx - win_w + 36.0; // +36 让面板右边略过图标中心，整体更靠右
        let max_x = mon_w - win_w - 4.0;
        if x > max_x {
            x = max_x;
        }
        if x < 4.0 {
            x = 4.0;
        }

        let y = {
            #[cfg(target_os = "macos")]
            {
                // macOS: 左上角原点，菜单栏在顶部（约 25pt）。
                // 面板顶部紧贴菜单栏底部。
                25.0
            }
            #[cfg(target_os = "windows")]
            {
                // Windows: 左上角原点，任务栏在底部。
                // 面板底部紧贴任务栏上方。
                mon_h - win_h - 48.0
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                mon_h - win_h - 48.0
            }
        };
        let _ = window.set_position(LogicalPosition::new(x, y));
    } else if let Some(mon) = window.current_monitor().ok().flatten() {
        // 无点击位置（如全局快捷键唤起）：定位到屏幕右上角（macOS）/右下角（Windows）
        let mon_w = mon.size().width as f64 / scale;
        #[cfg(not(target_os = "macos"))]
        let mon_h = mon.size().height as f64 / scale;

        let x = (mon_w - win_w - 4.0).max(4.0);
        let y = {
            #[cfg(target_os = "macos")]
            {
                25.0
            }
            #[cfg(target_os = "windows")]
            {
                mon_h - win_h - 48.0
            }
            #[cfg(not(any(target_os = "macos", target_os = "windows")))]
            {
                mon_h - win_h - 48.0
            }
        };
        let _ = window.set_position(LogicalPosition::new(x, y));
    }

    let _ = window.show();
    let _ = window.set_focus();
    // WKWebView 已知问题：长期隐藏的窗口 show 后首帧可能不立即重绘（白屏）。
    // 微调窗口尺寸强制触发 layer 重新提交（Tauri 社区验证的解法，见 issue #5170）。
    if let Ok(size) = window.outer_size() {
        let _ = window.set_size(PhysicalSize::new(size.width, size.height + 1));
        let _ = window.set_size(size);
    }
}

/// 格式化 token：3.7M / 1280
fn fmt_tok(n: i64) -> String {
    if n >= 1_000_000 {
        format!("{:.1}M", n as f64 / 1_000_000.0)
    } else if n >= 1_000 {
        format!("{:.1}K", n as f64 / 1_000.0)
    } else {
        n.to_string()
    }
}

/// 计算今日（自然日）的总花费 + 总 token，生成菜单栏标题文字。
fn today_tray_title(app: &AppHandle) -> String {
    // 当地时间今日 0 点
    let now = chrono::Local::now();
    let today_start = now
        .date_naive()
        .and_hms_opt(0, 0, 0)
        .unwrap()
        .and_local_timezone(chrono::Local)
        .single()
        .map(|d| d.timestamp_millis())
        .unwrap_or_else(|| now.timestamp_millis() - 86_400_000);
    let now_ms = now.timestamp_millis();

    let stats = db::query_stats(today_start, now_ms);
    let pricing = load_pricing().unwrap_or_default();
    // 货币偏好决定菜单栏显示 ¥ 还是 $；人民币花费 = 美元花费 × 当前汇率
    let is_usd = pricing::load_currency() == "usd";
    let fx = load_fx_rate();
    let to_display = |cost_usd: f64| if is_usd { cost_usd } else { cost_usd * fx };

    // 本机数据
    let mut total = stats.as_ref().map(|s| s.overall.total_tokens).unwrap_or(0);
    let mut cost = stats
        .as_ref()
        .map(|s| {
            s.by_model
                .iter()
                .map(|m| cost_for(m, &pricing.usd))
                .sum::<f64>()
        })
        .unwrap_or(0.0);

    // 多设备同步：合并远端（其他设备）今日数据
    let cfg = sync::load_sync_config().unwrap_or_default();
    if cfg.enabled && !cfg.device_token.is_empty() {
        let req = sync::RemoteUsageRequest {
            from_ms: today_start,
            to_ms: now_ms,
            bucket: "day".to_string(),
            exclude_device: cfg.device_id.clone(),
            devices: String::new(),
            source: String::new(),
        };
        // 远端请求失败时静默降级（服务器不可达不影响菜单栏显示）
        if let Ok(remote) = sync::fetch_remote_usage(req) {
            total += remote.overall.total_tokens;
            // 远端不含花费，按 pricing 自算
            cost += remote
                .by_model
                .iter()
                .map(|m| {
                    cost_for(
                        &db::ModelStat {
                            model_id: m.model_id.clone(),
                            provider_id: m.provider_id.clone(),
                            requests: m.requests,
                            input_tokens: m.input_tokens,
                            output_tokens: m.output_tokens,
                            cache_read_tokens: m.cache_read_tokens,
                            cache_write_tokens: m.cache_write_tokens,
                            reasoning_tokens: m.reasoning_tokens,
                            total_tokens: m.total_tokens,
                        },
                        &pricing.usd,
                    )
                })
                .sum::<f64>();
        }
    }

    // Cursor：合并今日用量（events 带 120s 缓存；未配置/未登录/失败静默降级，
    // 不影响 ZCode 部分展示，口径与前端汇总页一致：ZCode + Cursor 合计）
    if let Ok((cursor_cost_usd, cursor_tokens)) =
        cursor::fetch_cursor_usage_totals(today_start, now_ms)
    {
        total += cursor_tokens;
        cost += cursor_cost_usd;
    }

    // Codex：合并今日用量（本地导入库；未安装/失败静默降级，
    // 不影响 ZCode/Cursor 部分展示，口径与前端汇总页一致）
    if let Ok(stats) = codex::query_stats(today_start, now_ms) {
        total += stats.overall.total_tokens;
        cost += stats
            .by_model
            .iter()
            .map(|m| cost_for(m, &pricing.usd))
            .sum::<f64>();
    }

    // Claude：合并今日用量（本地导入库；未安装/失败静默降级，同上）
    if let Ok(stats) = claude::query_stats(today_start, now_ms) {
        total += stats.overall.total_tokens;
        cost += stats
            .by_model
            .iter()
            .map(|m| cost_for(m, &pricing.usd))
            .sum::<f64>();
    }

    let _ = app; // 占位
    let sym = if is_usd { "$" } else { "¥" };
    if total > 0 {
        format!("{sym}{:.2}  {}", to_display(cost), fmt_tok(total))
    } else {
        "ZBar".to_string()
    }
}

/// 阻止 App Nap：菜单栏常驻应用的面板窗口长期隐藏时，macOS 会判定应用空闲并
/// 挂起进程，WKWebView 的 WebContent 进程随之被挂起甚至回收——再次唤起面板时
/// 需整页重载（bundle + React 挂载 + 数据初始化），造成数秒白屏。
/// 启动时声明持续用户活动断言，让 WebView 在窗口隐藏期间保持存活。
/// 用 AllowingIdleSystemSleep 变体：阻止 App Nap 但允许系统空闲睡眠，兼顾笔记本电池。
#[cfg(target_os = "macos")]
fn prevent_app_nap() {
    use objc2_foundation::{NSActivityOptions, NSProcessInfo};

    let activity =
        NSProcessInfo::processInfo().beginActivityWithOptions_reason(
            NSActivityOptions::UserInitiatedAllowingIdleSystemSleep,
            &objc2_foundation::NSString::from_str("ZBar panel keep-alive"),
        );
    // 断言必须存活整个进程生命周期：forget 防止 drop 触发 endActivity 让断言失效
    std::mem::forget(activity);
}

/// 应用快捷键配置：先注销全部，再按配置注册。
fn apply_shortcut(app: &AppHandle, cfg: &shortcut::ShortcutConfig) -> Result<(), String> {
    use tauri_plugin_global_shortcut::GlobalShortcutExt;
    let gs = app.global_shortcut();
    // 先清掉旧注册，避免重复
    let _ = gs.unregister_all();
    if !cfg.enabled {
        return Ok(());
    }
    gs.on_shortcut(cfg.accelerator.as_str(), move |app, _shortcut, _event| {
        toggle_panel(app, None);
    })
    .map_err(|e| format!("注册快捷键失败（可能被占用或格式非法）: {e}"))?;
    Ok(())
}

/// 后台线程：每 30 秒刷新一次菜单栏标题。
fn spawn_title_updater(app: AppHandle) {
    std::thread::spawn(move || loop {
        let title = today_tray_title(&app);
        let _ = app.tray_by_id("main").map(|t| t.set_title(Some(title)));
        std::thread::sleep(std::time::Duration::from_secs(30));
    });
}

/// 后台线程：每天自动联网刷新一次 USD→CNY 汇率（写回 cursor 配置）。
/// 启动先 sleep 180s 错开启动网络高峰（项目已有 500ms/1500ms/2500ms 错峰惯例，
/// 汇率刷新更低频，无需抢启动窗口）。每轮重新读配置判断 fx_rate_auto——
/// 用户可能中途开关「每日自动更新汇率」，不能启动时读一次就用。
/// 获取失败不动现有汇率值，明日重试。
fn spawn_fx_rate_refresher() {
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_secs(180));
        loop {
            if let Ok(cfg) = cursor::load_cursor_config() {
                if cfg.fx_rate_auto {
                    if let Err(e) = cursor::fetch_fx_rate() {
                        eprintln!("[zbar-fx-rate] 每日汇率刷新失败（明日重试）: {e}");
                    }
                }
            }
            std::thread::sleep(std::time::Duration::from_secs(24 * 3600));
        }
    });
}

/// 初始化托盘图标
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, "quit", "退出 ZBar", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    // 初始标题只用占位文字：真实标题（今日花费 + token）的生成依次做
    // SQLite 查询、开启同步后的远程 HTTP、Cursor events 冷缓存全量分页拉取，
    // 冷启动网络慢时会把主线程卡数十秒。spawn_title_updater 启动后会在
    // 后台线程立即执行一次刷新（先刷新再 sleep），占位很快被真实标题替换。
    let title = "ZBar".to_string();
    let _tray = TrayIconBuilder::with_id("main")
        .icon(app.default_window_icon().unwrap().clone())
        .tooltip("ZBar · ZCode Token 监控")
        .title(title)
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| {
            if event.id().as_ref() == "quit" {
                app.exit(0);
            }
        })
        .on_tray_icon_event(|tray, event| {
            // 左键点击切换面板（mac 和 win 都用左键）
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                position,
                ..
            } = event
            {
                let scale = tray.app_handle().get_webview_window("panel")
                    .and_then(|w| w.scale_factor().ok())
                    .unwrap_or(1.0);
                let pos = (
                    position.x as f64 / scale,
                    position.y as f64 / scale,
                );
                toggle_panel(tray.app_handle(), Some(pos));
            }
        })
        .build(app)?;

    Ok(())
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_positioner::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(tauri_plugin_clipboard_manager::init())
        .on_window_event(|window, event| {
            // 失焦时自动隐藏面板（保留窗口本身，不销毁）
            if let WindowEvent::Focused(false) = event {
                if window.label() == "panel" {
                    // Windows 置顶常驻模式：读取持久化的 pin 状态，
                    // 若已置顶则跳过隐藏，让面板持续可见不被其它窗口遮挡。
                    if load_pin().unwrap_or(false) {
                        return;
                    }
                    let _ = window.hide();
                }
            }
        })
        .setup(|app| {
            // 注册官方开机自启插件：Windows 使用注册表，macOS 使用 LaunchAgent。
            // 插件只在桌面目标启用，移动端不参与构建。
            #[cfg(desktop)]
            {
                app.handle().plugin(tauri_plugin_autostart::init(
                    tauri_plugin_autostart::MacosLauncher::LaunchAgent,
                    None,
                ))?;
            }

            // macOS 隐藏 Dock 图标，只保留菜单栏
            #[cfg(target_os = "macos")]
            {
                app.set_activation_policy(tauri::ActivationPolicy::Accessory);

                // 显式清空窗口背景，避免 WebView 默认背景把原生 NSVisualEffectView
                // 盖成纯白；真正的底色和模糊由 windowEffects 的 popover 材质提供。
                if let Some(panel) = app.get_webview_window("panel") {
                    let _ = panel.set_background_color(Some(
                        tauri::window::Color(0, 0, 0, 0),
                    ));
                    tune_panel_vibrancy(&panel);
                }

                // 常驻活动断言：防止面板长期隐藏时应用被系统挂起、WebView 被回收
                prevent_app_nap();
            }
            setup_tray(app.handle())?;
            spawn_title_updater(app.handle().clone());
            spawn_fx_rate_refresher();

            // 应用全局快捷键配置（启动时若已启用则注册）
            let sc = shortcut::load_shortcut();
            if let Err(e) = apply_shortcut(app.handle(), &sc) {
                eprintln!("[zbar-shortcut] {e}");
            }

            // Windows 启动时恢复置顶常驻状态：若用户上次开启了置顶，
            // 启动后面板自动显示并常驻（不再因失焦隐藏）。
            // 用条件编译确保 macOS 完全不执行此分支。
            #[cfg(target_os = "windows")]
            {
                if load_pin().unwrap_or(false) {
                    if let Some(panel) = app.get_webview_window("panel") {
                        let _ = panel.set_always_on_top(true);
                        let _ = panel.show();
                        let _ = panel.set_focus();
                    }
                }
            }

            sync::spawn_sync_worker();
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_stats,
            list_models,
            get_pricing,
            set_pricing,
            get_currency,
            set_currency,
            check_pricing_updates,
            apply_pricing_updates,
            get_shortcut_config,
            set_shortcut_config,
            unregister_shortcut,
            fetch_quota,
            compute_cost,
            get_trend,
            save_report,
            get_sync_config,
            set_sync_config,
            register_device,
            sync_now,
            disconnect_device,
            remote_usage,
            remote_snapshots,
            remote_agent_quota_snapshots,
            list_remote_devices,
            get_cleanup_status,
            cleanup_server,
            merge_devices,
            rename_device,
            set_auto_cleanup,
            pending_upload_count,
            get_pin,
            set_pin,
            get_quota_history,
            get_weekly_compare,
            get_weekly_compare_for_snapshots,
            get_today_delta,
            get_agent_quota_history,
            clear_quota_history,
            get_compare_tokens,
            get_compare_tokens_for_agent,
            get_cursor_usage,
            get_cursor_config,
            set_cursor_config,
            fetch_fx_rate,
            get_codex_usage,
            get_codex_debug,
            get_claude_usage,
            get_claude_debug
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod tests {
    use super::*;

    fn info(provider: &str, model: &str) -> db::ModelInfo {
        db::ModelInfo {
            provider_id: provider.to_string(),
            model_id: model.to_string(),
        }
    }

    /// 多来源模型清单合并：跨来源同名去重、不同来源同名保留、按 model_id 排序
    #[test]
    fn merge_model_lists_dedupes_and_sorts() {
        let zcode = vec![info("zai", "glm-4.6"), info("zai", "glm-4.5-air")];
        let codex = vec![info("codex", "gpt-5.6-sol")];
        // 远端含本地已有的（同 provider+model → 去重）与本地没有的（保留）
        let remote = vec![info("zai", "glm-4.6"), info("claude", "claude-sonnet-4-5")];

        let merged = merge_model_lists(vec![zcode, codex, remote]);

        let ids: Vec<&str> = merged.iter().map(|m| m.model_id.as_str()).collect();
        assert_eq!(
            ids,
            vec!["claude-sonnet-4-5", "glm-4.5-air", "glm-4.6", "gpt-5.6-sol"],
            "按 model_id 排序且无重复"
        );
        // 同名不同来源不去重（cost_for 按模型 id 计价，表单按 model_id 再去重展示）
        assert_eq!(merged.len(), 4);
    }
}

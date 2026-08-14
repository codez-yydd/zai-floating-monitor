mod cursor;
mod db;
mod notify;
mod peak;
mod pricing;
mod quota;
mod quota_history;
mod shortcut;
mod sync;

use pricing::{load_pricing, save_pricing, ModelPrice, PricingConfig};
use quota::{load_quota, save_quota, QuotaConfig, QuotaResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalPosition, Manager, PhysicalSize, WindowEvent,
};

/// 计费所需的字段抽象。ModelStat 与 DailyModelStat 都实现它，
/// 这样 cost_for 可同时服务 compute_cost 和 get_daily_stats。
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
fn cost_for<B: Billable>(s: &B, map: &BTreeMap<String, ModelPrice>) -> f64 {
    map.get(s.model_id())
        .map(|p| {
            let non_cache_input =
                (s.input_tokens() - s.cache_read_tokens()).max(0) as f64;
            (non_cache_input * p.input
                + s.output_tokens() as f64 * p.output
                + s.cache_read_tokens() as f64 * p.cache_read)
                / 1_000_000.0
        })
        .unwrap_or(0.0)
}

/// get_stats 命令的入参
#[derive(Debug, Deserialize)]
struct StatsRequest {
    from_ms: i64,
    to_ms: i64,
}

/// get_stats：返回时间范围内的统计 + 按模型分组
#[tauri::command]
fn get_stats(req: StatsRequest) -> Result<db::Stats, String> {
    db::query_stats(req.from_ms, req.to_ms)
}

/// list_models：列出数据库中所有出现过的模型
#[tauri::command]
fn list_models() -> Result<Vec<db::ModelInfo>, String> {
    db::list_models()
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
#[tauri::command]
fn set_currency(currency: String, app: AppHandle) -> Result<(), String> {
    pricing::save_currency(&currency)?;
    let title = today_tray_title(&app);
    let _ = app.tray_by_id("main").map(|t| t.set_title(Some(title)));
    Ok(())
}

/// check_pricing_updates：对比内置默认价格表与用户当前配置，返回差异。
/// 仅用于"检查更新"提示，绝不自动覆盖。
/// 仅对「数据库出现过 ∪ 用户已手动配置」的模型做对比，避免提示用户没用过的模型。
#[tauri::command]
fn check_pricing_updates() -> Result<pricing::PricingDiff, String> {
    let user = load_pricing()?;
    // 相关模型 = 数据库里出现过的 + 用户已配置（任一货币）的
    let mut relevant: std::collections::HashSet<String> = std::collections::HashSet::new();
    db::list_models()?.into_iter().for_each(|m| {
        relevant.insert(m.model_id);
    });
    relevant.extend(user.cny.keys().cloned());
    relevant.extend(user.usd.keys().cloned());
    Ok(pricing::diff_with_defaults(&user, &relevant))
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

/// get_quota_config：读取额度查询配置（token + 端点）
#[tauri::command]
fn get_quota_config() -> Result<QuotaConfig, String> {
    load_quota()
}

/// set_quota_config：保存额度查询配置
#[tauri::command]
fn set_quota_config(config: QuotaConfig) -> Result<(), String> {
    save_quota(&config)
}

/// get_notify_config：读取额度预警配置
#[tauri::command]
fn get_notify_config() -> Result<notify::NotifyConfig, String> {
    Ok(notify::load_notify())
}

/// set_notify_config：保存额度预警配置
#[tauri::command]
fn set_notify_config(config: notify::NotifyConfig) -> Result<(), String> {
    notify::save_notify(&config)
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

/// fetch_quota：实时查询 Coding Plan 额度（5小时窗口 + 每周）
#[tauri::command]
fn fetch_quota() -> Result<QuotaResult, String> {
    let cfg = load_quota()?;
    quota::fetch_quota(&cfg)
}

// ===== 周额度追踪 / 对比页 / 高峰期 =====

/// 读取全部额度快照历史（按 ts 升序）。
#[tauri::command]
fn get_quota_history() -> Result<Vec<quota_history::QuotaSnapshot>, String> {
    quota_history::load_all()
}

/// 解析快照为"智谱重置周期"列表（对比页用）。
#[tauri::command]
fn get_weekly_compare() -> Result<Vec<quota_history::WeeklyPeriod>, String> {
    let snaps = quota_history::load_all()?;
    Ok(quota_history::split_periods(&snaps))
}

/// 今日增量：(增量百分比, 今日采样数)。
#[tauri::command]
fn get_today_delta() -> Result<(u32, u32), String> {
    quota_history::today_delta()
}

/// 清空额度快照历史（设置页"清理历史"用）。
#[tauri::command]
fn clear_quota_history() -> Result<(), String> {
    quota_history::clear_history()
}

/// 对比页"实际 token"列（本地部分）：对一组周期 [reset_at, end_at)
/// 逐周期聚合本地 model_usage 的 token。前端再合并远端。
#[tauri::command]
fn get_compare_tokens(
    periods: Vec<(i64, i64)>,
) -> Result<Vec<WeeklyTokenBucket>, String> {
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
}

/// 对比页"折算额度消耗"列（本地部分）：按订阅类型折算。
/// - V2：Σ token × 时段倍率（等效 token）
/// - V3：Σ 积分公式 × 时段倍率（积分）
/// - 都可再 × ZCode 优惠系数
/// 返回的 consumed 字段即折算后的消耗值。
#[tauri::command]
fn get_compare_consumed(
    periods: Vec<(i64, i64)>,
) -> Result<Vec<ConsumedBucket>, String> {
    let cfg = peak::load_peak().unwrap_or_default();
    let buckets = db::query_period_consumed(&periods, &cfg)?;
    Ok(buckets
        .into_iter()
        .map(|b| ConsumedBucket {
            reset_at: b.reset_at,
            end_at: b.end_at,
            consumed: b.consumed,
            requests: b.requests,
        })
        .collect())
}

/// 读取高峰期配置。
#[tauri::command]
fn get_peak_config() -> Result<peak::PeakConfig, String> {
    peak::load_peak()
}

/// 保存高峰期配置。
#[tauri::command]
fn set_peak_config(config: peak::PeakConfig) -> Result<(), String> {
    peak::save_peak(&config)
}

/// 切换订阅类型并重置为该类型的官方默认时段（保留 zcode_discount 设置）。
#[tauri::command]
fn set_plan_type(plan: peak::PlanType) -> Result<peak::PeakConfig, String> {
    let mut cfg = peak::load_peak().unwrap_or_default();
    cfg.reset_for_plan(plan);
    peak::save_peak(&cfg)?;
    Ok(cfg)
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
        cursor::fetch_cursor_snapshot(req.from_ms, req.to_ms)
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

/// 测试 Cursor 认证（设置页用）。返回 (email, name, membership_type)
/// 同样用 spawn_blocking 卸载网络 I/O。
#[tauri::command]
async fn test_cursor_auth() -> Result<(Option<String>, Option<String>, Option<String>), String> {
    tauri::async_runtime::spawn_blocking(|| cursor::test_cursor_auth())
        .await
        .map_err(|e| format!("Cursor 认证测试失败: {e}"))?
}

/// 诊断 Cursor events API（排查"暂无明细"问题）
#[tauri::command]
async fn cursor_debug() -> Result<cursor::CursorDebugInfo, String> {
    tauri::async_runtime::spawn_blocking(|| cursor::cursor_debug())
        .await
        .map_err(|e| format!("Cursor 诊断失败: {e}"))?
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

/// 对比页：单个周期的折算消耗结果。
#[derive(Debug, Serialize)]
struct ConsumedBucket {
    reset_at: i64,
    end_at: i64,
    /// 折算后的消耗（V2=等效token, V3=积分）
    consumed: f64,
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

#[tauri::command]
fn compute_cost(req: StatsRequest) -> Result<CostResult, String> {
    let stats = db::query_stats(req.from_ms, req.to_ms)?;
    let pricing = load_pricing().unwrap_or_default();

    let per_model_cny: Vec<ModelCost> = stats
        .by_model
        .iter()
        .map(|s| ModelCost {
            model_id: s.model_id.clone(),
            cost: cost_for(s, &pricing.cny),
        })
        .collect();
    let per_model_usd: Vec<ModelCost> = stats
        .by_model
        .iter()
        .map(|s| ModelCost {
            model_id: s.model_id.clone(),
            cost: cost_for(s, &pricing.usd),
        })
        .collect();

    Ok(CostResult {
        total_cny: per_model_cny.iter().map(|m| m.cost).sum(),
        total_usd: per_model_usd.iter().map(|m| m.cost).sum(),
        per_model_cny,
        per_model_usd,
    })
}

/// 打开配置目录（~/.zbar）
#[tauri::command]
fn open_config_dir() -> Result<(), String> {
    let dir = pricing::config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("{e}"))?;
    open::that(dir).map_err(|e| format!("打开目录失败: {e}"))
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
    MergeDevicesRequest, RemotePeriodDetail, RemotePeriodDetailRequest, RemoteSnapshot,
    RemoteSnapshotRequest, RemoteUsage, RemoteUsageRequest, RenameDeviceRequest, SyncConfig,
    SyncOutcome,
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

/// 向服务器注册设备（UI 填写 server_url + master_token + name 后调用）
#[tauri::command]
fn register_device(req: sync::RegisterRequest) -> Result<SyncConfig, String> {
    sync::register_device(req)
}

/// 手动触发一次增量上传
#[tauri::command]
fn sync_now() -> Result<SyncOutcome, String> {
    sync::upload_incremental()
}

/// 断开连接（清凭证，不删服务器数据）
#[tauri::command]
fn disconnect_device() -> Result<(), String> {
    sync::disconnect()
}

/// 拉取远端聚合数据（其他设备）
#[tauri::command]
fn remote_usage(req: RemoteUsageRequest) -> Result<RemoteUsage, String> {
    sync::fetch_remote_usage(req)
}

/// 拉取远端额度快照（带 device_id，供对比页/报告页跨设备周额度解析）
#[tauri::command]
fn remote_snapshots(req: RemoteSnapshotRequest) -> Result<Vec<RemoteSnapshot>, String> {
    sync::fetch_remote_snapshots(req)
}

/// 拉取远端各周期逐条用量明细（前端用本地 peak 配置折算消耗）
#[tauri::command]
fn remote_period_detail(
    req: RemotePeriodDetailRequest,
) -> Result<Vec<RemotePeriodDetail>, String> {
    sync::fetch_remote_period_detail(req)
}

/// 拉取设备列表
#[tauri::command]
fn list_remote_devices() -> Result<Vec<DeviceInfo>, String> {
    sync::fetch_devices()
}

/// 查询清理状态
#[tauri::command]
fn get_cleanup_status() -> Result<CleanupStatus, String> {
    sync::fetch_cleanup_status()
}

/// 执行服务端清理
#[tauri::command]
fn cleanup_server(req: CleanupServerRequest) -> Result<sync::CleanupResult, String> {
    sync::cleanup_server(req)
}

/// 合并设备：把来源设备数据并入目标设备后删除来源
#[tauri::command]
fn merge_devices(req: MergeDevicesRequest) -> Result<sync::MergeResult, String> {
    sync::merge_devices(req)
}

/// 修改设备显示名
#[tauri::command]
fn rename_device(req: RenameDeviceRequest) -> Result<sync::RenameResult, String> {
    sync::rename_device(req)
}

/// 配置服务端自动清理
#[tauri::command]
fn set_auto_cleanup(req: AutoCleanupServerRequest) -> Result<sync::AutoCleanupConfig, String> {
    sync::set_auto_cleanup(req)
}

/// 查询本机待上传的记录数（本机 max_rowid - 已上传游标），供同步面板显示。
#[tauri::command]
fn pending_upload_count() -> Result<i64, String> {
    let local_max = db::max_rowid()?;
    let cfg = sync::load_sync_config().unwrap_or_default();
    Ok((local_max - cfg.last_uploaded_rowid).max(0))
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
#[tauri::command]
fn get_trend(req: TrendRequest) -> Result<Vec<TrendBucket>, String> {
    let buckets = db::query_trend(req.from_ms, req.to_ms, &req.bucket)?;
    let pricing = load_pricing().unwrap_or_default();

    let out = buckets
        .into_iter()
        .map(|b| {
            let cost_cny = b
                .by_model
                .iter()
                .map(|m| cost_for(m, &pricing.cny))
                .sum::<f64>();
            let cost_usd = b
                .by_model
                .iter()
                .map(|m| cost_for(m, &pricing.usd))
                .sum::<f64>();
            TrendBucket {
                label: b.label,
                total_tokens: b.total_tokens,
                requests: b.requests,
                cost_cny,
                cost_usd,
            }
        })
        .collect();

    Ok(out)
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
    // 货币偏好决定菜单栏显示 ¥ 还是 $，以及用哪套价格表
    let cur = pricing::load_currency();
    let is_usd = cur == "usd";
    let price_map = if is_usd { &pricing.usd } else { &pricing.cny };

    // 本机数据
    let mut total = stats.as_ref().map(|s| s.overall.total_tokens).unwrap_or(0);
    let mut cost = stats
        .as_ref()
        .map(|s| {
            s.by_model
                .iter()
                .map(|m| cost_for(m, price_map))
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
                        price_map,
                    )
                })
                .sum::<f64>();
        }
    }

    let _ = app; // 占位
    let sym = if is_usd { "$" } else { "¥" };
    if total > 0 {
        format!("{sym}{:.2}  {}", cost, fmt_tok(total))
    } else {
        "ZBar".to_string()
    }
}

/// 应用快捷键配置：先注销全部，再按配置注册。
/// 注册的 handler 调用 toggle_panel 唤起/隐藏面板。
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

/// 初始化托盘图标
fn setup_tray(app: &AppHandle) -> tauri::Result<()> {
    let quit_item = MenuItem::with_id(app, "quit", "退出 ZBar", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&quit_item])?;

    let title = today_tray_title(app);
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
            }
            setup_tray(app.handle())?;
            spawn_title_updater(app.handle().clone());

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
            get_quota_config,
            set_quota_config,
            get_notify_config,
            set_notify_config,
            get_shortcut_config,
            set_shortcut_config,
            unregister_shortcut,
            fetch_quota,
            compute_cost,
            get_trend,
            open_config_dir,
            save_report,
            get_sync_config,
            set_sync_config,
            register_device,
            sync_now,
            disconnect_device,
            remote_usage,
            remote_snapshots,
            remote_period_detail,
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
            get_today_delta,
            clear_quota_history,
            get_compare_tokens,
            get_compare_consumed,
            get_peak_config,
            set_peak_config,
            set_plan_type,
            get_cursor_usage,
            get_cursor_config,
            set_cursor_config,
            test_cursor_auth,
            cursor_debug
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

mod db;
mod pricing;
mod quota;

use pricing::{load_pricing, save_pricing, ModelPrice, PricingConfig};
use quota::{load_quota, save_quota, QuotaConfig, QuotaResult};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, LogicalPosition, Manager, PhysicalSize, WindowEvent,
};

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

/// fetch_quota：实时查询 Coding Plan 额度（5小时窗口 + 每周）
#[tauri::command]
fn fetch_quota() -> Result<QuotaResult, String> {
    let cfg = load_quota()?;
    quota::fetch_quota(&cfg)
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

    fn cost_for(
        s: &db::ModelStat,
        map: &BTreeMap<String, ModelPrice>,
    ) -> f64 {
        map.get(&s.model_id)
            .map(|p| {
                // input_tokens 已包含 cache_read_tokens，缓存读部分按缓存价计费，
                // 剩余非缓存输入部分才按输入价计费。
                let non_cache_input =
                    (s.input_tokens - s.cache_read_tokens).max(0) as f64;
                (non_cache_input * p.input
                    + s.output_tokens as f64 * p.output
                    + s.cache_read_tokens as f64 * p.cache_read)
                    / 1_000_000.0
            })
            .unwrap_or(0.0)
    }

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
    }

    let _ = window.show();
    let _ = window.set_focus();
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

    let total = stats.as_ref().map(|s| s.overall.total_tokens).unwrap_or(0);
    let cost = stats.as_ref().map(|s| {
        let map = &pricing.cny;
        s.by_model
            .iter()
            .map(|m| {
                map.get(&m.model_id)
                    .map(|p| {
                        let non_cache_input =
                            (m.input_tokens - m.cache_read_tokens).max(0) as f64;
                        (non_cache_input * p.input
                            + m.output_tokens as f64 * p.output
                            + m.cache_read_tokens as f64 * p.cache_read)
                            / 1_000_000.0
                    })
                    .unwrap_or(0.0)
            })
            .sum::<f64>()
    }).unwrap_or(0.0);

    let _ = app; // 占位
    if total > 0 {
        format!("¥{:.2}  {}", cost, fmt_tok(total))
    } else {
        "ZBar".to_string()
    }
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
        .on_window_event(|window, event| {
            // 失焦时自动隐藏面板（保留窗口本身，不销毁）
            if let WindowEvent::Focused(false) = event {
                if window.label() == "panel" {
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
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_stats,
            list_models,
            get_pricing,
            set_pricing,
            get_quota_config,
            set_quota_config,
            fetch_quota,
            compute_cost,
            open_config_dir
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

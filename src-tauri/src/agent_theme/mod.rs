//! Agent 桌面动态壁纸（Rust 侧总装）。
//!
//! 面板作为遥控器，向 ZCode 桌面应用（Electron）注入动态视频壁纸主题：
//! asar 解包 → index.html 注入外链引用 → 重打包（外置清单动态收集）→
//! 校验 → 备份原 asar → 单会话替换（三级执行策略：进程直写优先，
//! 见 execute_replace_script）→ 拉起并验证存活。
//!
//! ## 平台差异（替换第⑧步的实现）
//!
//! - macOS：单会话 shell 脚本（copy 临时文件 → xattr 清属性 → codesign
//!   ad-hoc 重签名 → 原子 mv 换入，见 build_replace_script），三级策略
//!   直写 / 「应用管理」TCC 拦截识别 / osascript 提权兜底；
//! - Windows：**无 shell 脚本与签名步骤**（Windows 不校验应用签名），
//!   改为 Rust 原生文件操作（fs::copy 到同目录 incoming 临时名 →
//!   fs::rename 原子换入，见 windows_replace_asar），不变量与 macOS 一致：
//!   任何前置失败都不触碰原 asar；无写权限（Program Files 安装）时经
//!   PowerShell UAC 提权兜底（privilege.rs）执行同款 copy/move 序列。
//!
//! ## 替换架构不变量（勿破坏）
//!
//! 1. 主题注入是**纯追加式修改**（index.html 只追加外链引用行），唯一被
//!    替换的系统文件是 `app.asar` **这一个文件**；
//! 2. `app.asar.unpacked` 目录**永不被触碰**（不写入、不删除、不同步）——
//!    因为新 asar 的外置文件清单在打包时动态等于官方目录现状，官方目录
//!    天然满足新 asar 的引用；
//! 3. 外置清单**运行时动态收集**（官方 unpacked 目录的全部文件相对路径 +
//!    `**/*.node` 兜底），不枚举不判断文件类型——官方未来新增任何外置
//!    文件都自动适配，无需发版。
//!
//! 事故教训（旧版 rsync --delete 事故）：旧替换脚本用
//! `rsync -a --delete` 把新包 unpacked 同步进官方目录——glob 拆出不全时
//! `--delete` 先删后缺，把官方 spawn-helper 等外置文件误删，导致重装必败、
//! 终端功能受损，只能整体重装应用修复。新架构从源头消除该类事故：
//! 任何情况下都不写官方 unpacked 目录（见 build_replace_script）。
//!
//! 模块划分：
//! - store：主题目录 / params.json / state.json / variables.css
//! - inject：注入物模板与 index.html 幂等注入
//! - asar：npx @electron/asar 封装
//! - privilege：提权兜底（macOS osascript / Windows PowerShell UAC，
//!   含 macOS「应用管理」拦截识别与设置指引）
//! - sign：codesign ad-hoc 重签名（macOS 专属，签名命令已纳入替换/还原
//!   脚本内执行；Windows 无签名流程）
//! - usage_feed：对话页用量统计条数据源（turn_usage → usage-data.js，
//!   皮肤安装成功启动 / 卸载还原停止，见各流程挂点）

pub mod asar;
pub mod inject;
pub mod privilege;
pub mod sign;
pub mod store;
pub mod usage_feed;

use serde::Serialize;
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::time::Duration;
use tauri::{AppHandle, Emitter, Manager};
// Windows 下子进程统一走 accounts::run_hidden（CREATE_NO_WINDOW），
// 本模块不直接使用 Command（macOS 的 defaults/df 与直写 shell 除外）
#[cfg(not(windows))]
use std::process::Command;

/// 进度事件名（前端 listen 用）
pub const PROGRESS_EVENT: &str = "zbar://agent-theme-progress";
/// 安装所需最小磁盘剩余空间（约 1.2GiB：解包 + 重打包临时空间）
const MIN_FREE_BYTES: u64 = 1_288_490_188;
/// 启动后存活验证轮数（20 × 250ms = 5s）
const LAUNCH_POLL_COUNT: usize = 20;
/// 重启时退出确认轮数（20 × 250ms = 5s，与 LAUNCH_POLL_COUNT 同口径）。
/// quit 内部已含"优雅退出 → 轮询等待 → 强杀兜底 → 再轮询"完整序列并自带
/// 超时报错，此处轮询仅做二次确认，避免旧进程尚未完全退场就抢先拉起
const QUIT_POLL_COUNT: usize = 20;

// ============================================================
// 目标应用抽象
// ============================================================

/// 可注入壁纸主题的桌面应用。
pub trait AgentApp: Send + Sync {
    /// 应用标识（前端 invoke 参数，如 "zcode"）
    fn id(&self) -> &'static str;
    /// 展示名（错误信息用）
    fn display_name(&self) -> &'static str;
    /// 应用 bundle 安装路径
    fn app_bundle_path(&self) -> PathBuf;
    /// 应用内 asar 包路径
    fn asar_path(&self) -> PathBuf;
    /// asar 内渲染入口相对路径（正斜杠）
    fn renderer_entry_rel(&self) -> &'static str;
    /// 应用是否在运行
    fn running(&self) -> bool;
    /// 退出应用
    fn quit(&self) -> Result<(), String>;
    /// 启动应用（Err 携带诊断：候选探测结果或命令退出原因）
    fn launch(&self) -> Result<(), String>;
}

/// ZCode 桌面应用（Electron，bundle id dev.zcode.app）。
/// macOS 形如 /Applications/ZCode.app；Windows 为安装根目录（含
/// resources\app.asar 的上层，最常见 %LOCALAPPDATA%\Programs\ZCode）。
struct ZcodeApp;

impl AgentApp for ZcodeApp {
    fn id(&self) -> &'static str {
        "zcode"
    }

    fn display_name(&self) -> &'static str {
        "ZCode"
    }

    fn app_bundle_path(&self) -> PathBuf {
        #[cfg(not(windows))]
        {
            PathBuf::from("/Applications/ZCode.app")
        }
        #[cfg(windows)]
        {
            windows_install_root_or_default()
        }
    }

    fn asar_path(&self) -> PathBuf {
        #[cfg(not(windows))]
        {
            self.app_bundle_path().join("Contents/Resources/app.asar")
        }
        #[cfg(windows)]
        {
            self.app_bundle_path().join("resources").join("app.asar")
        }
    }

    fn renderer_entry_rel(&self) -> &'static str {
        "out/renderer/index.html"
    }

    fn running(&self) -> bool {
        #[cfg(any(target_os = "macos", windows))]
        {
            crate::accounts::zcode_running()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            false
        }
    }

    fn quit(&self) -> Result<(), String> {
        #[cfg(any(target_os = "macos", windows))]
        {
            crate::accounts::quit_zcode()
        }
        #[cfg(not(any(target_os = "macos", windows)))]
        {
            Err("当前平台暂不支持该操作（仅 macOS / Windows）".into())
        }
    }

    fn launch(&self) -> Result<(), String> {
        #[cfg(windows)]
        {
            if crate::accounts::launch_zcode() {
                Ok(())
            } else {
                // 分场景诊断：候选中存在真实 exe 却全部启动失败，多为安全
                // 软件/系统策略拦截；完全找不到 exe 才是安装位置未被候选
                // 覆盖，列出探测位置（前 3 条，超出截断加"等"）帮助定位
                let candidates = crate::accounts::zcode_exe_candidates();
                let existing: Vec<String> = candidates
                    .iter()
                    .filter(|p| p.is_file())
                    .map(|p| p.display().to_string())
                    .collect();
                Err(if existing.is_empty() {
                    let all: Vec<String> =
                        candidates.iter().map(|p| p.display().to_string()).collect();
                    let preview = if all.len() > 3 {
                        format!("{} 等", all[..3].join("、"))
                    } else {
                        all.join("、")
                    };
                    format!("未找到 ZCode.exe（已尝试 {} 处：{preview}）", all.len())
                } else {
                    format!(
                        "找到 ZCode.exe 但启动失败（{}），可能被安全软件或系统策略拦截",
                        existing.join("、")
                    )
                })
            }
        }
        #[cfg(target_os = "macos")]
        {
            match Command::new("open")
                .args(["-a", crate::accounts::ZCODE_APP_NAME])
                .output()
            {
                Ok(out) if out.status.success() => Ok(()),
                Ok(out) => {
                    // 退出码缺失（被信号终止）时给文字占位而非裸数字
                    let code = out
                        .status
                        .code()
                        .map(|c| c.to_string())
                        .unwrap_or_else(|| "未知".into());
                    let stderr = String::from_utf8_lossy(&out.stderr).trim().to_string();
                    if stderr.is_empty() {
                        Err(format!("open -a 退出码 {code}"))
                    } else {
                        Err(format!("open -a 退出码 {code}（{stderr}）"))
                    }
                }
                Err(e) => Err(format!("执行 open 失败：{e}")),
            }
        }
        #[cfg(not(any(windows, target_os = "macos")))]
        {
            Err("当前平台暂不支持该操作（仅 macOS / Windows）".into())
        }
    }
}

// ============================================================
// Windows 安装根目录探测（候选构造与校验函数已下沉 accounts：
// windows_install_candidates / registry_install_candidates /
// find_install_root，与 launch_zcode 的 exe 启动候选同源共用；
// 此处只保留探测入口与缓存/现场捕获一级候选）
// ============================================================

/// 一级候选：exe 路径缓存与运行中进程现场捕获。ZCode 可被 NSIS 安装器
/// 装到任意盘符（如 D:\app\ZCode），固定候选表覆盖不了；accounts 模块的
/// "退出 ZCode"机制本就会在进程存活时捕获并缓存 exe 路径，这里复用同一
/// 缓存：缓存命中直接取父目录；缓存缺失/失效且 ZCode 正在运行时现场
/// 捕获并回写（捕获时机从"仅退出时"扩展到"皮肤页探测时"，解决从未用过
/// 退出功能时的冷启动），之后每次打开皮肤页都走零开销的缓存路径。
#[cfg(windows)]
fn cache_or_captured_install_candidates() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(exe) = crate::accounts::zcode_exe_cached() {
        if let Some(dir) = exe.parent() {
            out.push(dir.to_path_buf());
        }
        return out;
    }
    // 缓存无效才付 PowerShell 捕获开销；未运行时捕获必失败，先短路
    if !crate::accounts::zcode_running() {
        return out;
    }
    if let Some(exe) = crate::accounts::capture_zcode_exe_path().filter(|p| p.is_file()) {
        crate::accounts::cache_zcode_exe_path(&exe);
        if let Some(dir) = exe.parent() {
            out.push(dir.to_path_buf());
        }
    }
    out
}

/// Windows 安装根目录探测入口：按"缓存/进程 → 注册表 → 固定候选"逐级
/// 构造候选并即取即校验（find_install_root 只认 `resources\app.asar`，
/// 判据唯一），命中即返回——缓存命中的常规路径零注册表枚举开销。全部
/// 未命中时退回最常见的用户级安装位置（与 macOS 固定路径同一展示语义：
/// 未安装时状态页报"未找到ZCode安装目录：<路径>"）。
#[cfg(windows)]
fn windows_install_root_or_default() -> PathBuf {
    let local = cache_or_captured_install_candidates();
    if let Some(hit) = crate::accounts::find_install_root(&local) {
        return hit;
    }
    let reg = crate::accounts::registry_install_candidates();
    if let Some(hit) = crate::accounts::find_install_root(&reg) {
        return hit;
    }
    let fixed = crate::accounts::windows_install_candidates();
    if let Some(hit) = crate::accounts::find_install_root(&fixed) {
        return hit;
    }
    std::env::var_os("LOCALAPPDATA")
        .map(|b| PathBuf::from(b).join("Programs").join("ZCode"))
        .unwrap_or_else(|| PathBuf::from(r"C:\Program Files\ZCode"))
}

/// 应用注册表（当前仅 ZCode，保留扩展位）
fn registry() -> &'static [Box<dyn AgentApp>] {
    static REGISTRY: OnceLock<Vec<Box<dyn AgentApp>>> = OnceLock::new();
    REGISTRY
        .get_or_init(|| vec![Box::new(ZcodeApp)])
        .as_slice()
}

fn find_app(app_id: &str) -> Option<&'static dyn AgentApp> {
    registry()
        .iter()
        .find(|a| a.id() == app_id)
        .map(|b| b.as_ref())
}

// ============================================================
// 进度事件
// ============================================================

/// 进度事件 payload（camelCase，契约一字不差）
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
struct ProgressPayload {
    app_id: String,
    stage: String,
    percent: f64,
    detail: Option<String>,
}

struct Progress {
    handle: AppHandle,
    app_id: String,
}

impl Progress {
    fn new(handle: &AppHandle, app_id: &str) -> Self {
        Self {
            handle: handle.clone(),
            app_id: app_id.to_string(),
        }
    }

    /// 发送进度事件（面板端未监听/已关闭时静默忽略）
    fn emit(&self, stage: &str, percent: f64, detail: Option<&str>) {
        let _ = self.handle.emit(
            PROGRESS_EVENT,
            ProgressPayload {
                app_id: self.app_id.clone(),
                stage: stage.to_string(),
                percent,
                detail: detail.map(|s| s.to_string()),
            },
        );
    }
}

// ============================================================
// 任务级互斥（防止安装/卸载并发执行）
// ============================================================

static TASK_MUTEX: Mutex<()> = Mutex::new(());

fn acquire_task_lock() -> Result<MutexGuard<'static, ()>, String> {
    TASK_MUTEX
        .try_lock()
        .map_err(|_| "已有安装/卸载任务正在进行，请等待完成后再试".to_string())
}

// ============================================================
// 辅助函数
// ============================================================

/// asar 对应的 unpacked 目录（app.asar → app.asar.unpacked）
fn asar_unpacked_of(asar: &Path) -> PathBuf {
    PathBuf::from(format!("{}.unpacked", asar.to_string_lossy()))
}

/// 递归统计目录下 .node 原生模块数量（目录不存在计 0）。
/// ⑥ 的计数校验已被集合包含校验（verify_unpacked_superset）取代，
/// 现仅单元测试用作断言辅助。
#[cfg_attr(not(test), allow(dead_code))]
fn count_node_files(dir: &Path) -> usize {
    if !dir.is_dir() {
        return 0;
    }
    let mut count = 0;
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(entries) = fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "node") {
                count += 1;
            }
        }
    }
    count
}

/// 递归收集目录下**全部常规文件**的相对路径（`/` 分隔，不带前导 `/`）。
/// 架构不变量 3 的清单来源：官方 unpacked 目录现状里有什么就收什么，
/// 不枚举、不判断文件类型——官方未来新增任何外置文件自动进入清单。
/// 目录不存在返回空清单；符号链接保守跳过（不跟随，防环）；
/// 隐藏文件/目录一并收集（不判断）。
fn collect_rel_files(dir: &Path) -> Vec<String> {
    let mut out = Vec::new();
    // 栈元素：(当前目录, 相对目录前缀)。用显式栈而非递归，避免深目录爆栈
    let mut stack = vec![(dir.to_path_buf(), String::new())];
    while let Some((cur, rel_prefix)) = stack.pop() {
        let Ok(entries) = fs::read_dir(&cur) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(file_type) = entry.file_type() else {
                continue;
            };
            let name = entry.file_name().to_string_lossy().to_string();
            let rel = if rel_prefix.is_empty() {
                name
            } else {
                format!("{rel_prefix}/{name}")
            };
            if file_type.is_dir() {
                stack.push((entry.path(), rel));
            } else if file_type.is_file() {
                out.push(rel);
            }
        }
    }
    out
}

/// minimatch glob 元字符转义：把路径段内会被解释为 glob 语法的字符前置
/// `\` 变为字面匹配（`\` 本身、通配符、字符组、brace、extglob 前缀等）。
/// `/` 分隔符保持原样——转义只作用于段内字符，不破坏路径结构。
/// @electron/asar 的 --unpack 经 minimatch（matchBase）匹配相对路径，
/// 官方 unpacked 里形如 `@scope/pkg/x.node`、`special (1)/y[w].dat`
/// 的文件名不经转义会被误解析导致拆出失败。
fn escape_glob(rel: &str) -> String {
    const META: &[char] = &[
        '\\', '*', '?', '[', ']', '{', '}', '(', ')', '!', '+', '@', '|', '^',
    ];
    let mut out = String::with_capacity(rel.len());
    for c in rel.chars() {
        if META.contains(&c) {
            out.push('\\');
        }
        out.push(c);
    }
    out
}

/// 由官方 unpacked 相对路径清单构造 asar pack 的 --unpack glob（纯函数，
/// 架构不变量 3 的落点）：
/// - 清单为空（官方无 unpacked 目录或目录为空）→ 退化为 `**/*.node`；
/// - 否则构造 brace glob：`{**/*.node,**/<rel1>,**/<rel2>,...}`——
///   minimatch 先做 brace 展开再逐子模式匹配，子模式含 `/` 合法；
/// - 清单去重 + 排序（输出确定，便于测试与日志比对）；
/// - 每个子模式固定带 `**/` 前缀以匹配任意层级，rel 段内 glob 元字符
///   经 escape_glob 转义为字面匹配；
/// - `**/*.node` 兜底永远保留：官方目录残缺/收集遗漏时仍保证原生模块
///   被拆出（多拆无害，缺拆必崩）。
fn build_unpack_glob(rels: &[String]) -> String {
    const NODE_FALLBACK: &str = "**/*.node";
    let mut unique: Vec<&str> = rels.iter().map(|s| s.as_str()).collect::<HashSet<_>>().into_iter().collect();
    if unique.is_empty() {
        return NODE_FALLBACK.to_string();
    }
    unique.sort_unstable();
    let subpatterns: Vec<String> = std::iter::once(NODE_FALLBACK.to_string())
        .chain(unique.into_iter().map(|rel| format!("**/{}", escape_glob(rel))))
        .collect();
    format!("{{{}}}", subpatterns.join(","))
}

/// 路径所在卷的剩余空间（字节）。非 macOS 平台直接视为充足。
fn available_disk_bytes(path: &Path) -> Result<u64, String> {
    #[cfg(target_os = "macos")]
    {
        let out = Command::new("df")
            .args(["-k", &path.to_string_lossy()])
            .output()
            .map_err(|e| format!("查询磁盘剩余空间失败: {e}"))?;
        let text = String::from_utf8_lossy(&out.stdout);
        // df -k 最后一行第 4 列为 Available（KB）
        let avail_kb = text
            .lines()
            .last()
            .and_then(|l| l.split_whitespace().nth(3))
            .and_then(|v| v.parse::<u64>().ok())
            .unwrap_or(0);
        Ok(avail_kb * 1024)
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = path;
        Ok(u64::MAX)
    }
}

/// 读取应用版本。
/// - macOS：Info.plist CFBundleShortVersionString（defaults read 优先，
///   失败直接解析 plist XML）；
/// - Windows：安装根目录下 ZCode.exe 的 ProductVersion（PowerShell 读取，
///   见 read_exe_product_version）；
/// - 读不到返回 None（现有调用方已容忍）。
pub fn read_app_version(bundle: &Path) -> Option<String> {
    #[cfg(windows)]
    {
        read_exe_product_version(&bundle.join("ZCode.exe"))
    }
    #[cfg(not(windows))]
    {
        let plist = bundle.join("Contents/Info.plist");
        if !plist.is_file() {
            return None;
        }
        #[cfg(target_os = "macos")]
        {
            let out = Command::new("defaults")
                .args([
                    "read",
                    &plist.to_string_lossy(),
                    "CFBundleShortVersionString",
                ])
                .output()
                .ok()?;
            if out.status.success() {
                let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !v.is_empty() {
                    return Some(v);
                }
            }
        }
        parse_plist_version(&plist)
    }
}

/// 读取 exe 的 ProductVersion（read_app_version 的 Windows 分支）。
/// 经 accounts::run_hidden 静默调用 PowerShell（CREATE_NO_WINDOW 防 GUI
/// 黑窗）；路径以单引号字面量嵌入（' → '' 转义，命令行按 UTF-16 传递，
/// 中文安装路径无损）。任何失败返回 None（版本指纹缺失时调用方放行）。
#[cfg(windows)]
fn read_exe_product_version(exe: &Path) -> Option<String> {
    if !exe.is_file() {
        return None;
    }
    let lit = exe.to_string_lossy().replace('\'', "''");
    let script = format!("(Get-Item '{lit}').VersionInfo.ProductVersion");
    let out = crate::accounts::run_hidden("powershell", &["-NoProfile", "-Command", &script])?;
    if !out.status.success() {
        return None;
    }
    let v = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 解析 plist XML 中 CFBundleShortVersionString 的值
/// （read_app_version 的非 Windows 分支使用）
#[cfg_attr(windows, allow(dead_code))]
fn parse_plist_version(plist: &Path) -> Option<String> {
    let text = fs::read_to_string(plist).ok()?;
    let key_pos = text.find("CFBundleShortVersionString")?;
    let after = &text[key_pos..];
    let s = after.find("<string>")? + "<string>".len();
    let e = after[s..].find("</string>")? + s;
    let v = after[s..e].trim().to_string();
    if v.is_empty() {
        None
    } else {
        Some(v)
    }
}

/// 安装过程中的 asar 解包临时目录名（zbar-theme-staging-<app>-<ts>）。
/// 注意：不仅目录名本身不能以点开头，**整条路径中严禁出现任何点开头的段**
/// （包括祖先目录，如 ~/.zbar 的 .zbar）——@electron/asar pack 的内部 glob
/// （--unpack "**/*.node"）按含全部祖先段的完整路径匹配，任何一个点开头段
/// 都会让 `**` 匹配失败，原生模块完全不被拆到 unpacked，安装校验报
/// "原生模块数量不一致"（实测踩坑，勿改回）。
fn staging_dir_name(app_id: &str, ts: i64) -> String {
    format!("zbar-theme-staging-{app_id}-{ts}")
}

/// 安装过程中的重打包临时文件名（zbar-theme-pack-<app>-<ts>.asar，约束同上）
fn pack_tmp_name(app_id: &str, ts: i64) -> String {
    format!("zbar-theme-pack-{app_id}-{ts}.asar")
}

/// 安装过程中的 asar 解包临时目录（系统临时目录下 zbar-theme-staging-<app>-<ts>）。
/// 必须放在系统临时目录（macOS /tmp、Windows %TEMP%）：路径不含点开头的
/// 隐藏祖先段，asar pack 的 unpack glob 才能正常匹配。
/// 严禁放回 ~/.zbar 或任何点开头目录之下（glob 对隐藏段不匹配，会导致
/// "原生模块数量不一致"安装失败，实测踩坑记录）。
fn staging_dir(app_id: &str, ts: i64) -> PathBuf {
    std::env::temp_dir().join(staging_dir_name(app_id, ts))
}

/// 安装过程中的重打包临时文件（系统临时目录下 zbar-theme-pack-<app>-<ts>.asar，
/// 位置约束同 staging_dir）
fn pack_tmp(app_id: &str, ts: i64) -> PathBuf {
    std::env::temp_dir().join(pack_tmp_name(app_id, ts))
}

/// 清理指定应用的全部 staging / pack 临时文件与目录（幂等，装前装后都调用）。
/// 新位置在系统临时目录（zbar-theme- 前缀）；同时一次性扫尾旧位置
/// ~/.zbar/agent-themes/ 下历史版本残留的两代旧前缀（zbar- 可见名与更早
/// 点开头的隐藏名），清理用户机器上的历史垃圾。
fn cleanup_staging(app_id: &str) -> Result<(), String> {
    let new_prefixes = vec![
        format!("zbar-theme-staging-{app_id}-"),
        format!("zbar-theme-pack-{app_id}-"),
    ];
    cleanup_staging_in(&std::env::temp_dir(), &new_prefixes)?;
    // 旧位置历史残留扫尾：themes_dir 不可用时跳过，不影响安装主流程
    if let Ok(themes) = store::themes_dir() {
        let legacy_prefixes = vec![
            format!("zbar-staging-{app_id}-"),
            format!("zbar-pack-{app_id}-"),
            format!(".staging-{app_id}-"),
            format!(".pack-{app_id}-"),
        ];
        cleanup_staging_in(&themes, &legacy_prefixes)?;
    }
    Ok(())
}

/// cleanup_staging 的实现（目录与前缀列表显式传入便于单元测试）：
/// 删除目录下以任一前缀开头的文件/目录，其余条目一律不动。
fn cleanup_staging_in(dir: &Path, prefixes: &[String]) -> Result<(), String> {
    if !dir.is_dir() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)
        .map_err(|e| format!("读取临时目录失败（{}）: {e}", dir.display()))?
    {
        let entry = entry.map_err(|e| format!("读取临时目录失败（{}）: {e}", dir.display()))?;
        let name = entry.file_name().to_string_lossy().to_string();
        if prefixes.iter().any(|p| name.starts_with(p)) {
            let path = entry.path();
            let removed = if path.is_dir() {
                fs::remove_dir_all(&path)
            } else {
                fs::remove_file(&path)
            };
            removed.map_err(|e| format!("清理临时文件失败（{}）: {e}", path.display()))?;
        }
    }
    Ok(())
}

/// app_id 白名单校验：所有以 app_id 拼路径落盘的命令入口必须先过本校验
/// （与 get_agent_theme_state 同款注册表校验），防止 `app_id="../x"` 之类
/// 输入把文件写到 ~/.zbar 之外（路径遍历）。
fn validate_app_id(app_id: &str) -> Result<(), String> {
    if find_app(app_id).is_some() {
        Ok(())
    } else {
        Err(format!("未知应用：{app_id}"))
    }
}

/// 把目录动态加入 asset 协议放行范围（壁纸库预览用，Tauri 2 API：
/// `app.asset_protocol_scope().allow_directory(dir, recursive)`）。
///
/// tauri.conf.json 的 assetProtocol.scope 静态为空，壁纸目录因用户/机器
/// 而异无法静态枚举，统一由本函数在运行时按实际目录动态放行
/// （recursive=true 覆盖用户壁纸目录的子层级）。放行失败仅记日志：
/// 预览图退化为前端占位徽章，不影响壁纸列表与切换功能。
fn allow_asset_dir(app: &AppHandle, dir: &Path) {
    if let Err(e) = app.asset_protocol_scope().allow_directory(dir, true) {
        println!(
            "[zbar] asset 协议放行壁纸目录失败（{}）: {e}",
            dir.display()
        );
    }
}

/// 第⑦步备份实现（路径显式化便于单元测试）：
/// - `current_injected=false`（当前 asar 未注入：首次安装 / 应用升级覆盖）→
///   备份当前 asar 到 backup_dir 并同步写 meta.json；目标同名文件已存在时
///   fs::copy 原地覆盖（幂等），随后统一执行旧版本备份清理；
/// - `current_injected=true`（重装场景，当前 asar 已含注入标记）→
///   跳过备份与 meta 覆写，沿用既有备份。注入版不是原版，无条件备份会让
///   latest_backup（meta 驱动）指向注入版——"还原原版"将还原出注入版，
///   且卸载会删除备份目录，真原版备份从此永久丢失。
/// 返回 true=已执行备份，false=因重装跳过。
fn write_backup_if_pristine(
    asar_path: &Path,
    backup_dir: &Path,
    version: Option<&str>,
    orig_size: u64,
    current_injected: bool,
) -> Result<bool, String> {
    if current_injected {
        return Ok(false);
    }
    fs::create_dir_all(backup_dir).map_err(|e| format!("创建备份目录失败：{e}"))?;
    let backup_path = backup_dir.join(store::backup_file_name(version, orig_size));
    fs::copy(asar_path, &backup_path).map_err(|e| format!("备份原 app.asar 失败: {e}"))?;
    store::write_backup_meta_in(backup_dir, orig_size, version.map(|v| v.to_string()))?;
    // 旧版本备份清理：meta.json 已指向刚写入的这份最新备份，其余
    // app.asar.v*.bak 纯占空间（每个约 300MB），且多版本备份共存曾让
    // 旧的 mtime 选择逻辑选错文件、还原被完整性校验拒绝（真机事故，
    // 见 store::latest_backup 复盘注释）→ 还原永远只应使用最新备份，
    // 备份成功后删掉其余旧版本。清理失败仅记日志不阻塞备份主流程：
    // 多留一份旧备份不影响 meta 驱动的正确选择。重装路径（上方提前
    // 返回）不做清理——此刻既有备份是唯一真原版，不可触碰。
    if let Err(e) = store::remove_stale_backups_in(backup_dir, &backup_path) {
        println!("[zbar] 清理旧版本备份失败（不影响备份与安装）: {e}");
    }
    Ok(true)
}

// ============================================================
// 提权脚本构建（单会话原子替换 / 还原）
// ============================================================

/// 提权脚本在原 asar 同目录使用的暂存文件路径（.zbar-<asar 文件名><后缀>，
/// 如 .../Resources/.zbar-app.asar.incoming / .zbar-app.asar.rollback）。
/// 点开头的隐藏名不与 Electron 自身的 app.asar.unpacked 等命名冲突，
/// 且与原 asar 同目录保证最终 mv 为同卷 rename（原子换入）。
fn asar_staging_sibling(asar: &Path, suffix: &str) -> PathBuf {
    let name = asar
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "app.asar".to_string());
    asar
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .join(format!(".zbar-{name}{suffix}"))
}

/// 构建"单会话原子替换"脚本（安装第⑧步，一次授权内整体执行）。
/// **macOS 专用**（shell 脚本 + xattr/codesign；Windows 无 shell 脚本与
/// 签名流程，改走 windows_replace_asar 的 Rust 原生文件操作，
/// Linux 无实际调用路径）。
///
/// 架构不变量（见模块头）：`app.asar` 是唯一被替换的系统文件，
/// `app.asar.unpacked` 永不被触碰——脚本因此**不含任何 unpacked
/// 同步/删除命令**：新 asar 的外置清单在 ⑤ 动态等于官方 unpacked 目录
/// 现状，官方目录天然满足新 asar 的引用，无需也不得同步。
///
/// 历史教训（勿回退）：旧版脚本曾用 `rsync -a --delete` 同步 unpacked，
/// glob 拆出不全时 `--delete` 先删后缺，官方 spawn-helper 等外置文件被
/// 误删，重装必败、终端功能受损，只能整体重装应用修复。
///
/// 设计动机（真机事故复盘）：旧实现分步提权——cp 直接覆盖原 asar，失败后
/// 重试、再失败触发还原又提权。macOS「应用管理」(App Management TCC) 拦截
/// 下不仅连环弹管理员密码框，且 cp 写到一半被拦会留下损坏的半截 asar，
/// 导致应用无法启动；随后的自动还原同样被拦，形成死局。
///
/// 新脚本把最危险的 asar 换入放在最后，任何前置失败原 asar 均未被动过：
/// ```sh
/// set -e
/// trap "rm -f <incoming>" EXIT     # 失败/成功路径统一清理临时文件
/// cp -f <packed> <incoming>        # A. 先写同目录临时文件（TCC 若拦截，
///                                  #    发生在动原文件之前，零损伤）
/// xattr -cr <bundle>               # B. 清隔离属性（quarantine 等）
/// codesign --force --deep --sign - <bundle>    # C. ad-hoc 重签名
/// codesign --verify --deep --strict <bundle>   #    + 校验
/// mv -f <incoming> <asar>          # D. 验证通过才原子换入（同卷 rename，
///                                  #    不存在"写到一半"的中间态）
/// ```
///
/// 脚本为单行（run_as_admin 约束不能含裸换行），步骤间用 && 连接，
/// 任一步失败整体以非零退出，原 asar 保持原状。
#[cfg(not(windows))]
fn build_replace_script(packed: &Path, asar: &Path, bundle: &Path) -> String {
    let incoming_q = privilege::sh_quote(asar_staging_sibling(asar, ".incoming"));
    let bundle_q = privilege::sh_quote(bundle);
    [
        "set -e".to_string(),
        // EXIT 统一清理临时文件（成功路径已被 mv 消费，rm -f 幂等）
        format!(r#"trap "rm -f {incoming_q}" EXIT"#),
        format!("cp -f {} {incoming_q}", privilege::sh_quote(packed)),
        format!("xattr -cr {bundle_q}"),
        format!("codesign --force --deep --sign - {bundle_q}"),
        format!("codesign --verify --deep --strict {bundle_q}"),
        // 最危险一步放最后：此前任何失败（含 TCC 拦截）原 asar 均未动，无需还原
        format!("mv -f {incoming_q} {}", privilege::sh_quote(asar)),
    ]
    .join(" && ")
}

/// 构建"单会话还原"脚本（卸载与安装失败自动回滚共用，一次授权）。
/// **macOS 专用**（Windows 还原走 windows_replace_asar 的同款原生序列，
/// 见 build_windows_elevate_cmd）。
/// 只操作 asar 本体（备份换入 + 签名决策），**无任何 unpacked 目录
/// 操作**（与替换脚本同一架构不变量：app.asar.unpacked 永不被触碰）。
///
/// ```sh
/// set -e
/// trap "rm -f <incoming> <rollback>" EXIT
/// cp -f <backup> <incoming>            # TCC 若拦截，发生在动原 asar 之前
/// xattr -cr <bundle>
/// mv -f <asar> <rollback>              # 还原前的 asar 先挪开暂存（而非
///                                      # cp 直接覆盖，避免中途失败写坏）
/// mv -f <incoming> <asar>              # 原子换入备份原版
/// { codesign --verify --deep --strict <bundle>        # 先验证现有签名
///   || { codesign --force --deep --sign - <bundle>    # 失败才 ad-hoc 重签
///        && codesign --verify --deep --strict <bundle> }
///   || { mv -f <rollback> <asar>; exit 1; } }          # 重签也失败：换回
///                                                      # 还原前现状并退出
/// ```
///
/// "先 verify 后签"的决策放进脚本（shell || 短路）保证一次会话完成；
/// 重签兜底也失败时把还原前的 asar 从 rollback 换回，现状不因还原失败
/// 而进一步破坏。
#[cfg(not(windows))]
fn build_restore_script(backup: &Path, asar: &Path, bundle: &Path) -> String {
    let incoming_q = privilege::sh_quote(asar_staging_sibling(asar, ".incoming"));
    let rollback_q = privilege::sh_quote(asar_staging_sibling(asar, ".rollback"));
    let asar_q = privilege::sh_quote(asar);
    let bundle_q = privilege::sh_quote(bundle);
    let verify = format!("codesign --verify --deep --strict {bundle_q}");
    let sign = format!("codesign --force --deep --sign - {bundle_q}");
    [
        "set -e".to_string(),
        format!(r#"trap "rm -f {incoming_q} {rollback_q}" EXIT"#),
        format!("cp -f {} {incoming_q}", privilege::sh_quote(backup)),
        format!("xattr -cr {bundle_q}"),
        // 还原前把当前 asar 挪开暂存，重签兜底失败时用于换回现状
        format!("mv -f {asar_q} {rollback_q}"),
        format!("mv -f {incoming_q} {asar_q}"),
        format!("{{ {verify} || {{ {sign} && {verify}; }} || {{ mv -f {rollback_q} {asar_q}; exit 1; }}; }}"),
    ]
    .join(" && ")
}

// ============================================================
// 脚本执行（三级策略：进程直写优先，提权兜底；macOS shell 脚本 /
// Windows Rust 原生文件操作）
// ============================================================

/// Resources 目录可写性探针结论。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum WriteProbe {
    /// 探针写入并删除成功：目录可写且「应用管理」TCC 已放行（或从未限制）。
    /// Windows 下（用户级安装目录）即普通可写结论
    Writable,
    /// EPERM：被「应用管理」TCC 拦截（首次弹窗被拒 / 曾拒绝过），脚本必然
    /// 同样被拦，直接上抛系统设置指引。
    /// Windows 无 TCC 机制，此分支不会触发（保留枚举变体与 macOS 共用）
    BlockedByTcc,
    /// EACCES / EROFS / ENOENT 等：普通权限不足（如 root/Administrator
    /// 安装的应用、Windows 的 Program Files），走提权兜底
    /// （macOS osascript / Windows PowerShell UAC）
    NeedPrivilege,
}

/// 探针写入失败的 errno 分类（独立成函数便于单元测试）：
/// - EPERM(1)：macOS「应用管理」TCC 拦截特征（Windows 的 Win32 错误码
///   体系里 1 为 ERROR_INVALID_FUNCTION，fs 写入不会返回，本分支无害）；
/// - 其余（EACCES=13 权限不足、EROFS=30 只读卷、ENOENT=2 目录不存在、
///   Windows ERROR_ACCESS_DENIED=5、非 unix 无 errno）：归入提权兜底，
///   由兜底路径报出真实错误。
fn classify_write_error(err: &std::io::Error) -> WriteProbe {
    const EPERM: i32 = 1;
    match err.raw_os_error() {
        Some(EPERM) => WriteProbe::BlockedByTcc,
        _ => WriteProbe::NeedPrivilege,
    }
}

/// 用「写入后立即删除」的探针文件检测目录可写性（比 access(W_OK) 可靠：
/// access 只查 DAC 权限位，探针能真实触发「应用管理」TCC 判定——首次会弹
/// 「ZBar 想要修改 ZCode」系统对话框，允许后探针成功且此后直写全放行；
/// Windows 下等价于真实写入测试，Program Files 等受保护目录归入
/// NeedPrivilege 走 UAC 兜底）。
fn probe_writable(dir: &Path) -> WriteProbe {
    let probe = dir.join(".zbar-write-probe");
    match fs::write(&probe, b"") {
        Ok(()) => {
            // 删除失败不影响可写结论（极少见），残留的隐藏探针文件无碍
            let _ = fs::remove_file(&probe);
            WriteProbe::Writable
        }
        Err(e) => classify_write_error(&e),
    }
}

/// 以当前进程身份直接执行单行脚本（不经 osascript 提权）。
/// 关键收益：脚本内 cp/xattr/codesign 的 TCC 责任方是 ZBar 自身，
/// 首次执行触发系统标准弹窗「ZBar 想要修改 ZCode」，允许后永久生效，
/// 且可在 系统设置 → 隐私与安全性 → 应用管理 中管理；而 osascript
/// 提权 shell 中的写入责任方无法归到 ZBar，会被静默拒绝且无处授权
/// （真机实测，详见模块头与 privilege.rs 注释）。
#[cfg(not(windows))]
fn run_script_direct(script: &str) -> Result<(), String> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(script)
        .output()
        .map_err(|e| format!("启动 shell 执行脚本失败：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        return Err(if stderr.is_empty() {
            format!("脚本执行失败（退出码 {:?}）", output.status.code())
        } else {
            format!("脚本执行失败：{stderr}")
        });
    }
    Ok(())
}

/// 待执行的应用文件替换操作（安装换入 / 备份还原）。
/// 把"执行什么"与"在哪个平台怎么执行"解耦：
/// - macOS/Linux：据此构造单会话 shell 脚本（build_replace_script /
///   build_restore_script，仅 macOS 实际可用）；
/// - Windows：直写走 Rust 原生文件操作（windows_replace_asar），
///   提权兜底构造 .cmd 序列（build_windows_elevate_cmd）。
#[derive(Debug, Clone, Copy)]
#[cfg_attr(windows, allow(dead_code))] // bundle 字段仅 macOS 脚本构造读取
enum ReplaceSpec<'a> {
    /// 安装第⑧步：用重打包的新 asar 换入原 asar
    Install {
        /// 源文件（重打包产物）
        src: &'a Path,
        /// 目标 asar（应用内 app.asar）
        asar: &'a Path,
        /// 应用 bundle / 安装根目录（macOS 签名用，Windows 忽略）
        bundle: &'a Path,
    },
    /// 卸载 / 安装失败自动回滚：用备份换回原 asar
    Restore {
        /// 源文件（备份的 app.asar）
        src: &'a Path,
        asar: &'a Path,
        bundle: &'a Path,
    },
}

impl ReplaceSpec<'_> {
    /// 源文件（新包 / 备份）与目标 asar——Windows 执行器
    /// （run_replace_direct / build_admin_script）共用；macOS 走完整
    /// 脚本构造（build_script_for）不经过本方法
    #[cfg(windows)]
    fn src_and_asar(&self) -> (&Path, &Path) {
        match self {
            ReplaceSpec::Install { src, asar, .. } | ReplaceSpec::Restore { src, asar, .. } => {
                (src, asar)
            }
        }
    }
}

/// 由操作描述构造 macOS 单会话 shell 脚本（直写与提权共用同一脚本，
/// 与既有行为一致：脚本内已含签名与失败自愈逻辑）。
#[cfg(not(windows))]
fn build_script_for(spec: &ReplaceSpec) -> String {
    match spec {
        ReplaceSpec::Install { src, asar, bundle } => build_replace_script(src, asar, bundle),
        ReplaceSpec::Restore { src, asar, bundle } => build_restore_script(src, asar, bundle),
    }
}

/// macOS 直写执行器：当前进程直接执行单会话脚本。
#[cfg(not(windows))]
fn run_replace_direct(spec: &ReplaceSpec) -> Result<(), String> {
    run_script_direct(&build_script_for(spec))
}

/// Windows 直写执行器：Rust 原生文件操作（无 shell 脚本与签名步骤）。
#[cfg(windows)]
fn run_replace_direct(spec: &ReplaceSpec) -> Result<(), String> {
    let (src, asar) = spec.src_and_asar();
    windows_replace_asar(src, asar)
}

/// Windows 提权脚本内容（.cmd 批处理，由 privilege::run_as_admin 写入
/// 临时文件后经 UAC 执行）。
#[cfg(windows)]
fn build_admin_script(spec: &ReplaceSpec) -> String {
    let (src, asar) = spec.src_and_asar();
    build_windows_elevate_cmd(src, asar)
}

/// cmd 批处理路径字面量：双引号包裹（引号内 `&` `|` `<` `>` `^` 等均为
/// 字面量），`%` 翻倍（%%）防止批处理变量展开。
#[cfg(windows)]
fn escape_cmd_path(p: &Path) -> String {
    format!("\"{}\"", p.to_string_lossy().replace('%', "%%"))
}

/// Windows 原生 asar 换入（Rust std 文件操作；Windows 不校验应用签名，
/// 无 xattr/codesign 步骤）。安装与还原共用。不变量与 macOS 脚本版一致：
/// 1. 先把源文件（新包 / 备份）复制到原 asar 同目录的 incoming 临时名
///    （`.zbar-app.asar.incoming`）——此前任何失败（磁盘满 / 权限不足）
///    都不触碰原 asar，零损伤，且失败路径清理半截临时文件；
/// 2. 最后 rename 原子换入：Rust std 在 Windows 用 MoveFileExW
///    (MOVEFILE_REPLACE_EXISTING) 实现 rename，可直接覆盖已存在文件，
///    同目录保证同卷（不存在"写到一半"的中间态）；
/// 3. rename 失败（目标被运行中进程锁定等极少见场景）回退
///    remove_file + rename，仍失败报中文错误——incoming 是源文件的完整
///    副本，按错误信息中的路径手动改名即可恢复，数据不丢。
#[cfg(windows)]
fn windows_replace_asar(src: &Path, asar: &Path) -> Result<(), String> {
    let incoming = asar_staging_sibling(asar, ".incoming");
    if let Err(e) = fs::copy(src, &incoming) {
        // 清理可能的半截临时文件（幂等）；原 asar 从未被触碰
        let _ = fs::remove_file(&incoming);
        return Err(format!(
            "写入临时替换文件失败（{} → {}）: {e}",
            src.display(),
            incoming.display()
        ));
    }
    // 最危险一步放最后：此前任何失败原 asar 均未动
    if let Err(first) = fs::rename(&incoming, asar) {
        // 回退：先删原文件再改名（REPLACE_EXISTING 失败的场景，如目标被占用）
        let _ = fs::remove_file(asar);
        if let Err(second) = fs::rename(&incoming, asar) {
            return Err(format!(
                "替换 app.asar 失败（{} → {}）：{first}；重试仍失败：{second}。\
                 完整副本已保留在 {}，可手动改名恢复",
                incoming.display(),
                asar.display(),
                incoming.display()
            ));
        }
    }
    Ok(())
}

/// Windows 提权 .cmd 内容（NeedPrivilege 兜底路径用）：步骤与
/// windows_replace_asar 一致——copy 到同目录临时名 → move 换入（失败
/// 回退 del + move），任何前置失败原 asar 均未被触碰；无签名步骤。
/// 首行 `chcp 65001` 配合 privilege 层"UTF-8 无 BOM 写入"保证脚本内
/// 中文路径被正确解析；换行由 privilege 层统一转为 CRLF。
#[cfg(windows)]
fn build_windows_elevate_cmd(src: &Path, asar: &Path) -> String {
    let src_q = escape_cmd_path(src);
    let incoming_q = escape_cmd_path(&asar_staging_sibling(asar, ".incoming"));
    let asar_q = escape_cmd_path(asar);
    [
        "@echo off",
        "chcp 65001 >nul",
        &format!("copy /Y {src_q} {incoming_q}"),
        "if errorlevel 1 exit /b 1",
        &format!("move /Y {incoming_q} {asar_q}"),
        "if errorlevel 1 goto fallback",
        "exit /b 0",
        ":fallback",
        &format!("del /f {asar_q}"),
        &format!("move /Y {incoming_q} {asar_q}"),
        "if errorlevel 1 exit /b 1",
        "exit /b 0",
    ]
    .join("\n")
}

/// 替换/还原的统一执行入口（三级策略，install/uninstall/restore 共用）：
/// 1. 直写（首选）：Resources 探针可写（用户安装的应用 owner 即当前用户，
///    TCC 已放行；Windows 用户级安装目录同理）→ 当前进程直接执行
///    （macOS 跑 shell 脚本，TCC 责任归 ZBar；Windows 跑原生文件操作）；
/// 2. 「应用管理」拦截（仅 macOS）：探针即报 EPERM（首次弹窗被拒 /
///    曾拒绝过）→ 脚本必然同样被拦，直接上抛带系统设置指引的文案
///    （不执行脚本）；
/// 3. 提权兜底：仅真的无写权限（如 root/Administrator 安装、Windows
///    Program Files）→ macOS 沿用 run_as_admin（osascript），Windows 走
///    PowerShell UAC，错误处理不变（含取消识别）。
/// 出口统一过 clarify_admin_error（仅命中 TCC 拦截特征时替换为指引文案，
/// 取消类与其余错误原样透传，调用方按 is_admin_cancelled 处理）。
fn execute_replace_script(
    app: &dyn AgentApp,
    spec: &ReplaceSpec,
    admin_prompt: &str,
) -> Result<(), String> {
    let resources = app
        .asar_path()
        .parent()
        .unwrap_or_else(|| Path::new("/tmp"))
        .to_path_buf();
    match probe_writable(&resources) {
        WriteProbe::Writable => run_replace_direct(spec).map_err(privilege::clarify_admin_error),
        WriteProbe::BlockedByTcc => Err(privilege::operation_not_permitted_hint()),
        WriteProbe::NeedPrivilege => {
            #[cfg(not(windows))]
            let script = build_script_for(spec);
            #[cfg(windows)]
            let script = build_admin_script(spec);
            privilege::run_as_admin(&script, admin_prompt)
                .map(|_| ())
                .map_err(privilege::clarify_admin_error)
        }
    }
}

// ============================================================
// 安装状态检测（三信号：注入标记 + 版本指纹 + 备份存在）
// ============================================================

/// 读取文件元信息里的修改时间并转为 Unix 秒（读取/转换失败返回 None，
/// 调用方的缓存判定随之退化为仅体积匹配，保持健壮）。
fn mtime_unix(meta: Option<&fs::Metadata>) -> Option<i64> {
    let modified = meta?.modified().ok()?;
    modified
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as i64)
}

/// 注入标记缓存命中判定（纯函数，供单测）：体积与 mtime 双匹配即信任
/// 缓存结论——不区分注入与否，覆盖"应用升级后注入失效（marker=false）"
/// 状态：该态此前缓存永不命中，每次打开皮肤页都实检 npx 抽检 1~3 秒，
/// 是皮肤页打开慢的根因。任一侧缺 mtime（旧版 state.json / 系统读取
/// 失败）时退化为仅体积匹配。
fn cache_hit(state: &store::StoredState, cur_size: u64, cur_mtime: Option<i64>) -> bool {
    if state.asar_size != Some(cur_size) {
        return false;
    }
    match (state.asar_mtime, cur_mtime) {
        (Some(cached), Some(cur)) => cached == cur,
        // 缺 mtime 的一侧无法比对 → 仅按体积判定（旧行为）
        _ => true,
    }
}

/// 实测当前 asar 是否含注入标记（带 state.json 缓存：体积与 mtime 双匹配
/// 则信任缓存，省去 npx 抽检开销）。
fn detect_injected(app: &dyn AgentApp, state: &mut store::StoredState, node_ok: bool) -> bool {
    let asar_path = app.asar_path();
    if !asar_path.is_file() {
        return false;
    }
    let cur_meta = fs::metadata(&asar_path).ok();
    let cur_size = cur_meta.as_ref().map(|m| m.len()).unwrap_or(0);
    let cur_mtime = mtime_unix(cur_meta.as_ref());
    // asar 体积与 mtime 双匹配：文件未变化，直接信任缓存结论（无论注入与否）
    if cache_hit(state, cur_size, cur_mtime) {
        return state.injected_marker;
    }
    if !node_ok {
        // 无法实检（Node.js 缺失）：退回缓存值
        return state.injected_marker;
    }
    let html = asar::asar_extract_file_to_stdout(&asar_path, app.renderer_entry_rel())
        .unwrap_or_default();
    let has = inject::has_inject(&html);
    // 回写缓存（检测结论跟随 asar 体积 + mtime 指纹，三者保持一致）
    if has != state.injected_marker || !cache_hit(state, cur_size, cur_mtime) {
        state.injected_marker = has;
        state.asar_size = Some(cur_size);
        state.asar_mtime = cur_mtime;
        let _ = store::save_state(app.id(), state);
    }
    has
}

/// get_agent_theme_state 的业务实现
fn state_impl(app_id: &str) -> Result<AgentThemeStateDto, String> {
    let Some(app) = find_app(app_id) else {
        return Err(format!("未知应用：{app_id}"));
    };
    // 模板版本升级检查（廉价文件读比对）：皮肤页每次查询状态时顺带把
    // 旧版 theme.css / effects.js 升级到当前内置版本。variables.css
    // 参数变化由注入的 effects.js 每秒热重载即时应用；模板文件本身的
    // 升级（theme.css / effects.js）经面板"重启 ZCode"冷启动完全重载
    // 生效。旧 asar 注入行（无 data 标记）无需重装主题。升级失败不
    // 阻断状态查询（面板可用性优先）。
    let _ = store::ensure_theme_assets(app.id(), None);
    let bundle = app.app_bundle_path();
    let bundle_exists = bundle.is_dir();
    let app_version = if bundle_exists {
        read_app_version(&bundle)
    } else {
        None
    };
    let node_ok = asar::node_available();
    let mut state = store::load_state(app.id());
    let installed = if bundle_exists {
        detect_injected(app, &mut state, node_ok)
    } else {
        false
    };
    let backup_exists = store::latest_backup(app.id()).is_some();
    // 版本指纹：应用升级会覆盖 asar，安装时的版本记录不再匹配当前版本
    let version_changed = app_version.is_some()
        && state.zcode_version.is_some()
        && app_version != state.zcode_version;
    let needs_reinstall = installed && version_changed;
    let backup_missing = installed && !backup_exists;

    let detail = if !bundle_exists {
        Some(format!(
            "未找到{}安装目录：{}",
            app.display_name(),
            bundle.display()
        ))
    } else if !node_ok {
        Some("未检测到 Node.js/npx，动态壁纸注入需要先安装 Node.js".to_string())
    } else if needs_reinstall {
        Some(format!(
            "{}已升级（{} → {}），需要重新安装动态壁纸主题",
            app.display_name(),
            state.zcode_version.as_deref().unwrap_or("?"),
            app_version.as_deref().unwrap_or("?")
        ))
    } else if backup_missing {
        Some("备份文件缺失，建议重新安装主题以重建备份".to_string())
    } else {
        None
    };

    Ok(AgentThemeStateDto {
        app_id: app.id().to_string(),
        installed,
        app_bundle_path: bundle_exists.then(|| bundle.to_string_lossy().to_string()),
        app_version,
        needs_reinstall,
        backup_missing,
        target_running: app.running(),
        node_available: node_ok,
        detail,
    })
}

// ============================================================
// 安装流程（10 步状态机）
// ============================================================

/// 归一化 asar list 输出的单条路径：去首尾空白与前导 `/`（list 行首可能
/// 带根目录前导斜杠），保证两侧清单入集合前格式统一。
fn normalize_list_path(p: &str) -> String {
    p.trim().trim_start_matches('/').to_string()
}

/// 文件清单集合比对（安装校验的核心校验）：对重打包 asar 重新 list，
/// 与解包前记录的原版清单集合比对，数量相等且集合差为零才通过。
///
/// 背景：官方 asar 与 @electron/asar 重打包的存储策略不同（填充/链接条目
/// 处理差异），重打包体积天然偏小约 9% 但内容无损，体积近似度不能作为
/// 内容完整性判据，清单集合逐一比对才是可靠校验。
fn verify_manifest(orig_list: &[String], packed: &Path) -> Result<(), String> {
    let orig: HashSet<String> = orig_list
        .iter()
        .map(|p| normalize_list_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    let new_list = asar::asar_list(packed)?;
    let new: HashSet<String> = new_list
        .iter()
        .map(|p| normalize_list_path(p))
        .filter(|p| !p.is_empty())
        .collect();
    if orig == new {
        return Ok(());
    }
    // 差异摘要：缺失 = 原版有而新包没有；多出 = 新包有而原版没有，
    // 各最多列 5 条辅助定位，全量差异以日志为准的场景不存在（就地报错）
    let fmt_diff = |items: &[&String]| -> String {
        items.iter().take(5).map(|s| s.as_str()).collect::<Vec<_>>().join("、")
    };
    let missing: Vec<&String> = orig.difference(&new).collect();
    let extra: Vec<&String> = new.difference(&orig).collect();
    let mut msg = format!(
        "新 asar 文件清单与原版不一致（原 {} 项 / 新 {} 项）",
        orig.len(),
        new.len()
    );
    if !missing.is_empty() {
        msg.push_str(&format!("，缺失：{}", fmt_diff(&missing)));
    }
    if !extra.is_empty() {
        msg.push_str(&format!("，多出：{}", fmt_diff(&extra)));
    }
    msg.push_str("，已中止安装");
    Err(msg)
}

/// 安装⑥的 unpacked 集合校验（取代旧版 .node 计数比对）：
/// pack 产物 `<dest>.unpacked` 的文件集合必须 **⊇ 官方 unpacked 目录
/// 现有文件集合**（集合包含比对，而非计数）。
/// - 官方目录现有文件逐一必须在 pack 产物拆出集合内，任一缺失即中止
///   安装并列出缺失文件——这保证替换后新 asar 引用的每个外置文件都在
///   官方目录存在（官方目录不被触碰，见架构不变量 2）；
/// - 官方目录残缺（本身缺文件）时**不放大缺失**：只校验官方现有的，
///   官方没有的不做要求；
/// - pack 产物多拆出的文件（`**/*.node` 兜底）不视为错误。
fn verify_unpacked_superset(orig_unpacked: &Path, new_unpacked: &Path) -> Result<(), String> {
    let orig: HashSet<String> = collect_rel_files(orig_unpacked).into_iter().collect();
    let new: HashSet<String> = collect_rel_files(new_unpacked).into_iter().collect();
    let mut missing: Vec<&str> = orig.difference(&new).map(|s| s.as_str()).collect();
    if missing.is_empty() {
        return Ok(());
    }
    missing.sort_unstable();
    // 差异摘要最多列 5 条辅助定位（与 verify_manifest 风格一致）
    let shown = missing.iter().take(5).copied().collect::<Vec<_>>().join("、");
    let suffix = if missing.len() > 5 { " 等" } else { "" };
    Err(format!(
        "新 asar 外置文件不完整（官方 unpacked 现有 {} 个文件，重打包缺失 {} 个：{}{suffix}），已中止安装",
        orig.len(),
        missing.len(),
        shown
    ))
}

/// 安装主流程。`replaced` 标记原 asar 是否已被提权脚本原子换入：
/// 换入之前的失败（含提权脚本自身失败——此时原 asar 从未被触碰）
/// 只需清临时目录并复位状态即净；换入之后的失败（启动验证不过）
/// 必须用备份还原。
fn install_steps(
    app: &dyn AgentApp,
    prog: &Progress,
    resource_wallpapers: Option<&Path>,
    staging: &Path,
    packed: &Path,
    replaced: &mut bool,
) -> Result<(), String> {
    // 全程持锁：防止并发安装/卸载交叉破坏 asar
    let _guard = acquire_task_lock()?;

    // ---------- ① 预检 ----------
    prog.emit("precheck", 2.0, None);
    let bundle = app.app_bundle_path();
    if !bundle.is_dir() {
        return Err(format!(
            "未找到{}安装目录：{}",
            app.display_name(),
            bundle.display()
        ));
    }
    let asar_path = app.asar_path();
    if !asar_path.is_file() {
        return Err(format!("未找到应用资源包：{}", asar_path.display()));
    }
    if !asar::node_available() {
        return Err("未检测到 Node.js/npx，请先安装 Node.js 后重试".into());
    }
    let themes = store::themes_dir()?;
    fs::create_dir_all(&themes).map_err(|e| format!("创建主题目录失败：{e}"))?;
    // 磁盘预检针对 staging/pack 临时目录所在卷（系统临时目录，解包 + 重打包
    // 的大头开销在这里）；~/.zbar 只存主题资产与备份，体积小，无需按此预检
    let free = available_disk_bytes(&std::env::temp_dir())?;
    if free < MIN_FREE_BYTES {
        return Err(format!(
            "磁盘剩余空间不足：注入约需 1.2GB 临时空间，当前可用 {:.1}GB",
            free as f64 / 1_000_000_000.0
        ));
    }
    let mut state = store::load_state(app.id());
    if state.is_installing_recent() {
        return Err("上一次安装仍在进行或刚异常中断，请稍后重试".into());
    }
    state.status = Some(store::STATUS_INSTALLING.to_string());
    state.installing_since = Some(chrono::Utc::now().timestamp());
    store::save_state(app.id(), &state).map_err(|e| format!("写入安装状态失败: {e}"))?;

    // 清掉上次异常残留的临时目录
    cleanup_staging(app.id())?;

    // ---------- ② 退出目标应用（前端已弹窗确认） ----------
    prog.emit("quit", 6.0, None);
    if app.running() {
        app.quit()
            .map_err(|e| format!("退出{}失败：{e}", app.display_name()))?;
    }

    // ---------- ③ 解包 ----------
    prog.emit("extract", 10.0, Some("正在准备主题资源（约 1~2 分钟）"));
    // 清单基线：解包前先对原 asar 跑一次 list。@electron/asar 重打包与官方
    // 构建的存储策略不同，体积天然偏小约 9% 且内容无损，完整性校验以文件
    // 清单集合比对为准（见 ⑥），体积窗口仅作灾难兜底。
    let orig_manifest = asar::asar_list(&asar_path)
        .map_err(|e| format!("读取原版文件清单失败：{e}"))?;
    asar::asar_extract(&asar_path, staging)?;

    // ---------- ④ 注入 + 首次落盘主题资产 ----------
    prog.emit("inject", 30.0, None);
    store::ensure_theme_assets(app.id(), resource_wallpapers)?;
    let index_html = staging.join(app.renderer_entry_rel());
    if !index_html.is_file() {
        return Err(format!(
            "解包结果中未找到渲染入口 {}，已中止安装",
            app.renderer_entry_rel()
        ));
    }
    inject::apply_inject(&index_html, &store::app_dir(app.id())?)?;

    // ---------- ⑤ 重新打包（外置清单动态等于官方 unpacked 现状） ----------
    prog.emit("pack", 35.0, Some("正在应用主题（约 1~2 分钟）"));
    // 架构不变量 3：运行时收集官方 unpacked 目录全部文件相对路径构造
    // --unpack glob（+`**/*.node` 兜底）。官方目录现状里有什么就外置什么，
    // 不枚举不判断文件类型——官方未来新增任何外置文件自动适配；
    // 官方目录不存在/为空时清单为空，glob 退化为 `**/*.node`。
    let orig_unpacked = asar_unpacked_of(&asar_path);
    let unpack_rels = collect_rel_files(&orig_unpacked);
    let unpack_glob = build_unpack_glob(&unpack_rels);
    asar::asar_pack_with_unpack(staging, packed, &unpack_glob)?;

    // ---------- ⑥ 打包校验 ----------
    prog.emit("verify", 55.0, Some("正在校验文件完整性"));
    let orig_size = fs::metadata(&asar_path)
        .map_err(|e| format!("读取原 asar 失败: {e}"))?
        .len();
    let new_size = fs::metadata(packed)
        .map_err(|e| format!("读取新 asar 失败: {e}"))?
        .len();
    // 体积窗口 [0.75x, 1.5x] 仅作灾难兜底（asar 写坏/空包/嵌套打包等）：
    // @electron/asar 重打包与官方构建存储策略不同，体积天然偏小约 9% 属
    // 正常差异，内容完整性以下方文件清单集合比对为准
    if new_size < (orig_size as f64 * 0.75) as u64 || new_size > (orig_size as f64 * 1.5) as u64 {
        return Err(format!(
            "新 asar 体积异常（原 {orig_size} 字节 / 新 {new_size} 字节，超出合理范围），已中止安装"
        ));
    }
    // 核心校验：文件清单集合比对（数量相等且集合差为零才通过）
    verify_manifest(&orig_manifest, packed)?;
    let probe = asar::asar_extract_file_to_stdout(packed, app.renderer_entry_rel())
        .map_err(|e| format!("新 asar 抽检失败: {e}"))?;
    if !inject::has_inject(&probe) {
        return Err("新 asar 中未检测到注入标记，已中止安装".into());
    }
    // unpacked 集合校验（取代旧版 .node 计数比对）：新包拆出集合必须
    // ⊇ 官方 unpacked 现有文件集合——官方目录不被触碰，新 asar 引用的
    // 每个外置文件都必须已在官方目录存在；官方残缺时不放大缺失
    let new_unpacked = asar_unpacked_of(packed);
    verify_unpacked_superset(&orig_unpacked, &new_unpacked)?;

    // ---------- ⑦ 备份原 asar（目标在本机 Home，免提权） ----------
    let version = read_app_version(&bundle);
    // 重装守卫：当前 asar 已注入（实检 ZBAR-THEME 标记，体积未变时信任 state
    // 缓存）时它不是原版，无条件备份会污染备份目录、真原版永久丢失。
    // 已注入 → 跳过备份与 meta 覆写，沿用既有备份；
    // 未注入（首次安装 / 应用升级覆盖后的原版 asar）→ 执行备份。
    let current_injected = detect_injected(app, &mut state, true);
    let backup_dir = store::backup_dir(app.id())?;
    if write_backup_if_pristine(
        &asar_path,
        &backup_dir,
        version.as_deref(),
        orig_size,
        current_injected,
    )? {
        prog.emit("backup", 65.0, None);
    } else {
        prog.emit("backup", 65.0, Some("检测到重装，保留原始备份"));
    }

    // ---------- ⑧ 单会话替换：三级策略（进程直写优先；macOS 一次执行内"临时落盘 → 清属性 → 签名 → 原子换入"，Windows 原生"copy → rename 原子换入"） ----------
    // 真机实测结论（macOS）：/Applications 下用户安装的应用 Resources 目录
    // 普通用户本就可写；此前的 osascript 管理员提权路径在 macOS「应用管理」
    // (App Management TCC) 下是死结——提权 shell 中 cp 的责任方无法归到
    // ZBar，被静默拒绝且系统设置里无授权项。现由 ZBar 进程直接执行替换
    // （execute_replace_script 三级策略），TCC 责任归 ZBar：首次触发系统
    // 弹窗「ZBar 想要修改 ZCode」，允许后永久生效；仅真的无写权限
    // （root 安装）才回退提权。Windows 同策略：用户级安装目录直写，
    // Program Files 无写权限时走 PowerShell UAC 兜底。两平台的执行序列
    // 自身不变量一致：任何一步失败（含 TCC 拦截）时原 asar 均未被触碰，
    // 系统零损伤——直接上抛，绝不重试、绝不触发还原。
    // 替换只动 asar 本体，不含任何 unpacked 操作（架构不变量 2，
    // 新 asar 外置清单已动态等于官方目录现状，见 ⑤）
    prog.emit(
        "replace",
        75.0,
        Some("正在替换应用文件（首次可能弹出系统确认框，请点允许）"),
    );
    let replace_prompt = format!("ZBar：向{}注入动态壁纸主题", app.display_name());
    let replace_spec = ReplaceSpec::Install {
        src: packed,
        asar: &asar_path,
        bundle: &bundle,
    };
    if let Err(e) = execute_replace_script(app, &replace_spec, &replace_prompt) {
        // 用户取消（兜底路径）/ TCC 拦截（已转为系统设置指引）/ 其它失败：
        // 原 asar 未被动过，一律直接中止上抛（不重试、不还原，避免连环弹授权窗）
        return Err(e);
    }
    // 只有替换成功原子换入后才算"已替换"，此后失败（启动验证不过）才需要还原
    *replaced = true;
    // 签名与校验（仅 macOS）已随脚本完成，此处仅推进进度
    prog.emit("sign", 85.0, None);

    // ---------- ⑨ 清理临时文件 + 写安装状态 ----------
    prog.emit("cleanup", 92.0, None);
    cleanup_staging(app.id())?;
    let final_meta = fs::metadata(&asar_path)
        .map_err(|e| format!("读取替换后 asar 失败: {e}"))?;
    let done_state = store::StoredState {
        status: Some(store::STATUS_INSTALLED.to_string()),
        zcode_version: version,
        asar_size: Some(final_meta.len()),
        asar_mtime: mtime_unix(Some(&final_meta)),
        injected_at: Some(chrono::Utc::now().to_rfc3339()),
        injected_marker: true,
        installing_since: None,
    };
    store::save_state(app.id(), &done_state).map_err(|e| format!("写入安装状态失败: {e}"))?;

    // ---------- ⑩ 拉起 + 存活验证 ----------
    prog.emit("launch", 95.0, None);
    if let Err(reason) = app.launch() {
        // 先还原再报错：保证用户拿到错误时应用文件已回到原版
        restore_backup(app, prog)?;
        return Err(format!(
            "无法启动{}：{reason}（已自动还原备份）",
            app.display_name()
        ));
    }
    for _ in 0..LAUNCH_POLL_COUNT {
        if app.running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    restore_backup(app, prog)?;
    Err(format!(
        "{}启动后未能稳定运行（已自动还原备份）",
        app.display_name()
    ))
}

/// 安装命令包装：临时目录命名、失败收尾与进度收口。
fn install_impl(
    handle: &AppHandle,
    app_id: &str,
    resource_wallpapers: Option<PathBuf>,
) -> Result<(), String> {
    let Some(app) = find_app(app_id) else {
        return Err(format!("未知应用：{app_id}"));
    };
    let prog = Progress::new(handle, app_id);
    let ts = chrono::Utc::now().timestamp();
    let staging = staging_dir(app_id, ts);
    let packed = pack_tmp(app_id, ts);
    let mut replaced = false;
    let result = install_steps(
        app,
        &prog,
        resource_wallpapers.as_deref(),
        &staging,
        &packed,
        &mut replaced,
    );
    // 无论成败兜底清理临时目录（成功路径在 cleanup 阶段已清，此处幂等）
    let _ = cleanup_staging(app_id);
    match result {
        Ok(()) => {
            prog.emit("done", 100.0, Some("动态壁纸主题安装完成"));
            // 皮肤已就绪：启动用量统计条数据源（usage-data.js 周期导出）
            usage_feed::start();
            Ok(())
        }
        Err(e) => {
            // ⑧ 步替换脚本失败时原 asar 从未被换入（replaced=false）：磁盘上
            // 无应用改动，复位 installing 状态即净；replaced=true 的失败
            // （启动验证不过）由 restore_backup 内部复位状态；用户在还原授权
            // 中取消时应用文件保持已注入状态，同样复位 installing 标记，
            // 避免残留状态在 INSTALLING_STALE_SECS 内拦截用户重试
            // （实际安装状态以实检为准）
            if !replaced || privilege::is_admin_cancelled(&e) {
                store::reset_state(app_id);
            }
            prog.emit("error", 100.0, Some(&e));
            Err(e)
        }
    }
}

// ============================================================
// 还原流程（卸载 / 安装失败自动回滚共用）
// ============================================================

/// 用备份还原原 asar（单会话替换：一次授权内完成换入 + macOS 的
/// "先验证后重签"决策 + 失败自愈换回；Windows 为原生 copy → rename
/// 序列），成功后复位状态。
fn restore_backup(app: &dyn AgentApp, prog: &Progress) -> Result<(), String> {
    prog.emit(
        "replace",
        88.0,
        Some("安装失败，正在自动还原备份（可能弹出系统确认框，请点允许）"),
    );
    let asar_path = app.asar_path();
    let bundle = app.app_bundle_path();
    let Some((backup, meta)) = store::latest_backup(app.id()).map(|b| {
        let m = store::load_backup_meta(app.id());
        (b, m)
    }) else {
        return Err("未找到 app.asar 备份，无法自动还原（可重装 ZCode 应用修复）".into());
    };
    // 备份完整性：体积必须与备份记录一致
    let size = fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
    if let Some(m) = &meta {
        if m.asar_size != size {
            return Err(format!(
                "备份文件不完整（记录 {} 字节 / 实际 {size} 字节），无法自动还原",
                m.asar_size
            ));
        }
    }
    // 还原意味着皮肤即将卸载/失效：先停用量导出线程再动 asar
    usage_feed::stop();
    let _ = app.quit();
    // 单会话替换（三级策略执行）：cp 备份到临时名 → 换入 → 先验证签名、
    // 失败才 ad-hoc 重签、再失败在脚本内换回还原前状态（Windows 为同款
    // 原生 copy → rename 序列，无签名步骤）
    let restore_spec = ReplaceSpec::Restore {
        src: &backup,
        asar: &asar_path,
        bundle: &bundle,
    };
    match execute_replace_script(app, &restore_spec, "ZBar：还原应用原始文件") {
        Ok(_) => {}
        // 用户取消管理员授权（兜底路径）：原样上抛，不重试（避免连环弹授权窗）
        Err(e) if privilege::is_admin_cancelled(&e) => return Err(e),
        // 其余失败（含 TCC 拦截 → 已转为设置指引）：已保证现状未被破坏
        Err(e) => {
            return Err(format!(
                "自动还原失败：{e}（可手动重装 ZCode 应用修复）"
            ));
        }
    }
    store::reset_state(app.id());
    Ok(())
}

/// 卸载命令的业务实现。
fn uninstall_impl(handle: &AppHandle, app_id: &str) -> Result<(), String> {
    let Some(app) = find_app(app_id) else {
        return Err(format!("未知应用：{app_id}"));
    };
    let prog = Progress::new(handle, app_id);
    let result = (|| -> Result<(), String> {
        let _guard = acquire_task_lock()?;

        // 备份完整性校验（还原的唯一数据源，先验后动）
        prog.emit("precheck", 2.0, None);
        let bundle = app.app_bundle_path();
        if !bundle.is_dir() {
            return Err(format!(
                "未找到{}安装目录：{}",
                app.display_name(),
                bundle.display()
            ));
        }
        let Some(backup) = store::latest_backup(app.id()) else {
            return Err("未找到原始 app.asar 备份，无法还原（可重装 ZCode 应用修复）".into());
        };
        let meta = store::load_backup_meta(app.id());
        let size = fs::metadata(&backup).map(|m| m.len()).unwrap_or(0);
        if let Some(m) = &meta {
            if m.asar_size != size {
                return Err(format!(
                    "备份文件不完整（记录 {} 字节 / 实际 {size} 字节），已中止卸载",
                    m.asar_size
                ));
            }
        }

        // 版本一致性校验：ZCode 升级后备份仍是旧版 asar，此时还原会把旧 asar
        // 覆盖进新 bundle，主进程/渲染层版本错配可能导致应用无法启动，必须拒绝。
        // 仅用户主动卸载时校验；安装中途失败的自动还原发生在同版本内，不受影响。
        // 任一侧读不到版本（旧版 meta / plist 异常）时无从比对，放行交由体积校验兜底。
        let app_version = read_app_version(&bundle);
        if let Some(m) = &meta {
            if let (Some(backup_v), Some(cur_v)) = (&m.zcode_version, &app_version) {
                if backup_v != cur_v {
                    return Err(format!(
                        "备份与应用版本不一致（备份 v{backup_v} / 当前 v{cur_v}），\
                         请先重新安装主题以重建备份"
                    ));
                }
            }
        }

        // 退出目标应用（前端已弹窗确认）
        prog.emit("quit", 8.0, None);
        if app.running() {
            app.quit()
                .map_err(|e| format!("退出{}失败：{e}", app.display_name()))?;
        }

        // 单会话还原（三级策略执行：进程直写优先，弹的是系统确认框
        // 而非管理员密码框；换入备份 + "先验证后重签"决策 + 失败自愈换回
        // 均在脚本内一次执行完成；Windows 为同款原生 copy → rename 序列）
        prog.emit(
            "replace",
            20.0,
            Some("正在还原原始文件（首次可能弹出系统确认框，请点允许）"),
        );
        let asar_path = app.asar_path();
        let restore_spec = ReplaceSpec::Restore {
            src: &backup,
            asar: &asar_path,
            bundle: &bundle,
        };
        match execute_replace_script(app, &restore_spec, "ZBar：还原应用原始文件") {
            Ok(_) => {}
            // 用户取消管理员授权（兜底路径）：直接中止，不重试（避免连环弹授权窗）
            Err(e) if privilege::is_admin_cancelled(&e) => return Err(e),
            // 其余失败（含 TCC 拦截 → 已转为设置指引）：已保证现状未被破坏
            Err(e) => {
                return Err(format!(
                    "还原 app.asar 失败：{e}（可手动重装 ZCode 应用修复）"
                ));
            }
        }
        // 签名校验/重签已随脚本完成，此处仅推进进度
        prog.emit("sign", 60.0, None);

        // 皮肤即将卸载：先停用量导出线程（避免其在目录清理后重建目录写
        // usage-data.js；usage-data.js 随主题目录清理一并删除）
        usage_feed::stop();
        // 清理主题目录（wallpapers 壁纸素材保留）+ 状态复位
        prog.emit("cleanup", 80.0, None);
        store::cleanup_theme_dir_keep_wallpapers(app.id())?;
        store::reset_state(app.id());
        Ok(())
    })();

    match result {
        Ok(()) => {
            prog.emit("done", 100.0, Some("已还原 ZCode 原始外观，壁纸素材已保留"));
            Ok(())
        }
        Err(e) => {
            prog.emit("error", 100.0, Some(&e));
            Err(e)
        }
    }
}

// ============================================================
// Tauri 命令（前端契约：参数/返回一字不差，serde camelCase）
// ============================================================

/// get_agent_theme_state 返回结构
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AgentThemeStateDto {
    pub app_id: String,
    pub installed: bool,
    pub app_bundle_path: Option<String>,
    pub app_version: Option<String>,
    pub needs_reinstall: bool,
    pub backup_missing: bool,
    pub target_running: bool,
    pub node_available: bool,
    pub detail: Option<String>,
}

/// set_agent_wallpaper 返回结构
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperInfo {
    pub file_name: String,
}

/// list_agent_wallpapers 返回项。
/// `path`：壁纸唯一标识——默认项为 "default"，其余为绝对路径
/// （同时是 select_agent_wallpaper 的入参与 params.wallpaperFile 的存值）
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct WallpaperEntryDto {
    pub path: String,
    /// 文件名（默认项为 default.mp4，前端以专属词条展示为"默认流光"）
    pub file_name: String,
    /// "video" | "image"
    pub kind: String,
    /// 预览源绝对路径：默认项指向 wallpapers/ 下的 default.mp4（path 的
    /// "default" 只是逻辑标识，前端预览需要真实文件路径），其余等于 path。
    /// 前端经 convertFileSrc 转 asset:// URL 供预览卡加载（放行链路见
    /// allow_asset_dir）。
    pub preview_path: String,
}

/// 查询目标应用的注入状态（三信号合成：注入标记 + 版本指纹 + 备份存在）。
#[tauri::command]
pub async fn get_agent_theme_state(app_id: String) -> Result<AgentThemeStateDto, String> {
    tauri::async_runtime::spawn_blocking(move || state_impl(&app_id))
        .await
        .map_err(|e| format!("状态检测任务失败：{e}"))?
}

/// 安装动态壁纸主题（emit 进度事件 zbar://agent-theme-progress）。
#[tauri::command]
pub async fn install_agent_theme(app_id: String, app: AppHandle) -> Result<(), String> {
    // 应用打包资源内的 wallpapers/（默认壁纸，可能尚未产出，缺失时跳过拷贝）
    let resource_wallpapers = app
        .path()
        .resolve("wallpapers", tauri::path::BaseDirectory::Resource)
        .ok()
        .filter(|p| p.is_dir());
    tauri::async_runtime::spawn_blocking(move || install_impl(&app, &app_id, resource_wallpapers))
        .await
        .map_err(|e| format!("安装任务失败：{e}"))?
}

/// 卸载并还原 ZCode 原始外观（wallpapers 壁纸素材保留）。
#[tauri::command]
pub async fn uninstall_agent_theme(app_id: String, app: AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || uninstall_impl(&app, &app_id))
        .await
        .map_err(|e| format!("卸载任务失败：{e}"))?
}

/// 读取主题参数（无 params.json 时返回默认值）。
#[tauri::command]
pub async fn get_agent_theme_params(app_id: String) -> Result<store::ThemeParams, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径读写，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        Ok(store::load_params(&app_id))
    })
    .await
    .map_err(|e| format!("读取参数任务失败：{e}"))?
}

/// 保存主题参数并重渲 variables.css（ZCode 运行中由注入的 effects.js
/// 每秒热重载变量，改参数无需重启应用即生效）。
#[tauri::command]
pub async fn set_agent_theme_params(
    app_id: String,
    params: store::ThemeParams,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径落盘，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        store::save_params(&app_id, &params)?;
        // ensure：模板版本升级检查 + 按最新参数重渲 variables.css
        store::ensure_theme_assets(&app_id, None)
    })
    .await
    .map_err(|e| format!("保存参数任务失败：{e}"))?
}

/// 导入壁纸文件（视频或图片）到 wallpapers/（返回落盘文件名）。换壁纸
/// 指向后由 effects.js 热重载即时生效（换源时媒体重置占位态再淡入，
/// 不闪白；V3 起图片与视频同样支持）。
#[tauri::command]
pub async fn set_agent_wallpaper(
    app_id: String,
    src_path: String,
) -> Result<WallpaperInfo, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径落盘，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        // 模板版本升级检查：确保新壁纸指向被最新版 effects.js 消费（热重载）
        store::ensure_theme_assets(&app_id, None)?;
        let src = PathBuf::from(&src_path);
        if !src.is_file() {
            return Err(format!("壁纸文件不存在：{src_path}"));
        }
        let file_name = src
            .file_name()
            .ok_or("无法解析壁纸文件名")?
            .to_string_lossy()
            .to_string();
        // 仅接受视频/图片白名单扩展名，防止误导入任意文件
        if store::wallpaper_kind_of(&file_name).is_none() {
            return Err(
                "仅支持 mp4 / webm / mov 视频与 jpg / jpeg / png / webp 图片".into(),
            );
        }
        let wp_dir = store::wallpapers_dir(&app_id)?;
        fs::create_dir_all(&wp_dir).map_err(|e| format!("创建壁纸目录失败: {e}"))?;
        fs::copy(&src, wp_dir.join(&file_name))
            .map_err(|e| format!("拷贝壁纸文件失败: {e}"))?;
        // 换壁纸全链路在本命令内闭环：拷贝成功后立即把新文件名写入 params.json
        // 的 wallpaperFile 并重渲 variables.css，CSS 变量即指向新壁纸。
        // （此前只拷文件不更新指向，重渲仍读旧文件名，导致拖入新壁纸后一切如旧）
        store::apply_wallpaper(&app_id, &file_name)?;
        Ok(WallpaperInfo { file_name })
    })
    .await
    .map_err(|e| format!("导入壁纸任务失败：{e}"))?
}

/// 列出壁纸库全部可选项（内置默认项固定第一 + wallpapers/ 目录平铺 +
/// 用户壁纸目录全递归，非默认项按文件名排序）。每次调用都会把内置
/// wallpapers/ 与用户壁纸目录动态放行进 asset 协议（allow_asset_dir），
/// 前端皮肤页加载/刷新列表时即完成放行，预览卡可直接转 asset:// 加载。
#[tauri::command]
pub async fn list_agent_wallpapers(
    app_id: String,
    app: AppHandle,
) -> Result<Vec<WallpaperEntryDto>, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径读目录，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        let dir = store::app_dir(&app_id)?;
        let wp_dir = store::wallpapers_dir(&app_id)?;
        // asset 协议放行：内置 wallpapers/ 目录 + 用户壁纸目录（设置了且存在时）
        allow_asset_dir(&app, &wp_dir);
        if let Some(user_dir) = store::load_params(&app_id)
            .wallpaper_dir
            .as_deref()
            .map(Path::new)
            .filter(|p| p.is_dir())
        {
            allow_asset_dir(&app, user_dir);
        }
        let mut out = vec![WallpaperEntryDto {
            path: "default".to_string(),
            file_name: store::DEFAULT_WALLPAPER_FILE.to_string(),
            kind: "video".to_string(),
            preview_path: wp_dir.join(store::DEFAULT_WALLPAPER_FILE).to_string_lossy().to_string(),
        }];
        for file in store::list_wallpapers_in(&dir, &wp_dir) {
            let file_name = file
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default();
            // 扫描已按扩展名过滤，此处 kind 必有值；兜底按视频防脏数据
            let kind = store::wallpaper_kind_of(&file_name).unwrap_or("video");
            out.push(WallpaperEntryDto {
                path: file.to_string_lossy().to_string(),
                preview_path: file.to_string_lossy().to_string(),
                file_name,
                kind: kind.to_string(),
            });
        }
        Ok(out)
    })
    .await
    .map_err(|e| format!("读取壁纸列表任务失败：{e}"))?
}

/// 从壁纸库选中壁纸（热重载约 1 秒生效，无需重启目标应用）。
/// `path == "default"` 指向回落 default.mp4；其余必须位于 wallpapers/ 或
/// 用户壁纸目录内（canonicalize 前缀比对，防任意路径注入）。
#[tauri::command]
pub async fn select_agent_wallpaper(app_id: String, path: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径落盘，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        // 模板版本升级检查：确保选中的图片壁纸被 V3 effects.js 支持
        store::ensure_theme_assets(&app_id, None)?;
        let dir = store::app_dir(&app_id)?;
        let wp_dir = store::wallpapers_dir(&app_id)?;
        store::select_wallpaper_in(&dir, &wp_dir, &path)
    })
    .await
    .map_err(|e| format!("切换壁纸任务失败：{e}"))?
}

/// 设置/清除用户壁纸目录（壁纸库的扫描来源之一）。
/// `dir` 为空串时清除（None）；否则必须校验为存在的目录（canonicalize
/// 落盘，保证后续 select 的前缀比对与列表扫描口径一致）。
/// 设置成功后把该目录动态放行进 asset 协议（allow_asset_dir），新目录的
/// 预览图无需等待下一次列表刷新即可加载。
#[tauri::command]
pub async fn set_agent_wallpaper_dir(
    app_id: String,
    dir: String,
    app: AppHandle,
) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        // app_id 白名单：store 层会以 app_id 拼路径落盘，必须先校验防路径遍历
        validate_app_id(&app_id)?;
        let raw = dir.trim();
        if raw.is_empty() {
            return store::set_wallpaper_dir(&app_id, None);
        }
        // canonicalize 后剥 Windows verbatim 前缀，避免 \\?\C:\… 形态
        // 落盘污染后续列表扫描与前缀比对（与 store 侧口径一致）
        let canon = inject::strip_verbatim_prefix(
            Path::new(raw)
                .canonicalize()
                .map_err(|_| format!("壁纸目录不存在：{raw}"))?,
        );
        if !canon.is_dir() {
            return Err(format!("不是有效的壁纸目录：{raw}"));
        }
        store::set_wallpaper_dir(&app_id, Some(canon.to_string_lossy().to_string()))?;
        allow_asset_dir(&app, &canon);
        Ok(())
    })
    .await
    .map_err(|e| format!("设置壁纸目录任务失败：{e}"))?
}

/// 重启目标应用（退出 → 等待 1s → 拉起），参数调整后生效用。
#[tauri::command]
pub async fn restart_target_app(app_id: String) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(app) = find_app(&app_id) else {
            return Err(format!("未知应用：{app_id}"));
        };
        app.quit()?;
        std::thread::sleep(Duration::from_secs(1));
        app.launch().map_err(|e| format!("启动{}失败：{e}", app.display_name()))
    })
    .await
    .map_err(|e| format!("重启任务失败：{e}"))?
}

/// restart_zcode 返回结构：restarted = 是否执行了完整重启（false =
/// 目标原本未在运行，仅直接拉起，前端按两种结果分别提示）。
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct RestartResultDto {
    pub restarted: bool,
}

/// 重启 ZCode 桌面应用：注入的 theme.css / effects.js 依赖应用冷启动加载，
/// 参数热重载（variables.css 每秒轮询）覆盖不到注入文件本身的改动（如用户
/// 手动编辑过注入文件、模板大版本升级），需要整进程重启才能完全重载。
/// 未运行时直接拉起；运行中退出（quit 内部含优雅退出与强杀兜底，失败直接
/// 报错）→ 轮询二次确认退出 → 拉起。
#[tauri::command]
pub async fn restart_zcode(app_id: String) -> Result<RestartResultDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        // 注册表白名单（与安装/卸载一致）：未知应用直接报错，不走文件路径
        let Some(app) = find_app(&app_id) else {
            return Err(format!("未知应用：{app_id}"));
        };
        let name = app.display_name();
        if !app.running() {
            app.launch().map_err(|e| format!("启动{name}失败：{e}"))?;
            return Ok(RestartResultDto { restarted: false });
        }
        app.quit().map_err(|e| format!("退出{name}失败：{e}"))?;
        for _ in 0..QUIT_POLL_COUNT {
            if !app.running() {
                app.launch().map_err(|e| format!("启动{name}失败：{e}"))?;
                return Ok(RestartResultDto { restarted: true });
            }
            std::thread::sleep(Duration::from_millis(250));
        }
        Err(format!("等待{name}退出超时，请手动退出后重试"))
    })
    .await
    .map_err(|e| format!("重启任务失败：{e}"))?
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_包含_zcode() {
        let app = find_app("zcode").expect("注册表应包含 zcode");
        assert_eq!(app.id(), "zcode");
        assert_eq!(app.display_name(), "ZCode");
        // 平台专属路径断言：macOS/Linux 为固定 bundle 路径；
        // Windows 为动态探测（见下方 windows_tests 的探测纯函数测试）
        #[cfg(not(windows))]
        {
            assert_eq!(
                app.app_bundle_path(),
                PathBuf::from("/Applications/ZCode.app")
            );
            assert_eq!(
                app.asar_path(),
                PathBuf::from("/Applications/ZCode.app/Contents/Resources/app.asar")
            );
        }
        #[cfg(windows)]
        {
            let asar = app.asar_path();
            assert!(
                asar.ends_with(r"resources\app.asar"),
                "Windows asar 应位于安装根目录 resources 下：{}",
                asar.display()
            );
        }
        assert_eq!(app.renderer_entry_rel(), "out/renderer/index.html");
        assert!(find_app("not-exist").is_none());
    }

    #[test]
    fn unpacked_路径推导() {
        assert_eq!(
            asar_unpacked_of(Path::new("/a/app.asar")),
            PathBuf::from("/a/app.asar.unpacked")
        );
    }

    #[test]
    fn 注入缓存判定_体积加mtime双匹配才信任() {
        // 核心回归：升级后"注入失效"态（marker=false 且体积匹配）也命中
        // 缓存——直接回 false，不再每次打开皮肤页都实检 npx 抽检
        let state = store::StoredState {
            asar_size: Some(284_000_000),
            asar_mtime: Some(1_770_000_000),
            injected_marker: false,
            ..Default::default()
        };
        assert!(cache_hit(&state, 284_000_000, Some(1_770_000_000)));
        // 体积不一致（应用升级替换了 asar）→ 实检
        assert!(!cache_hit(&state, 284_000_001, Some(1_770_000_000)));
        // 体积相同但 mtime 变化（内容被触碰）→ 实检
        assert!(!cache_hit(&state, 284_000_000, Some(1_770_000_001)));

        // 旧版 state.json 未记录 mtime（None）→ 退化为仅体积匹配（旧行为）
        let legacy = store::StoredState {
            asar_size: Some(284_000_000),
            injected_marker: true,
            ..Default::default()
        };
        assert!(cache_hit(&legacy, 284_000_000, Some(1_770_000_000)));
        assert!(cache_hit(&legacy, 284_000_000, None));
        assert!(!cache_hit(&legacy, 284_000_001, None));

        // 当前 mtime 读取失败（None）→ 同样退化为仅体积匹配，保持健壮
        assert!(cache_hit(&state, 284_000_000, None));
        assert!(!cache_hit(&state, 284_000_001, None));
    }

    #[test]
    fn mtime_unix_元信息缺失或转换失败返回none() {
        assert_eq!(mtime_unix(None), None);
        // 常规文件的 mtime 必然晚于 Unix 纪元 → 正常转秒
        let dir = std::env::temp_dir().join(format!("zbar-mtime-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let file = dir.join("a");
        fs::write(&file, b"x").unwrap();
        let meta = fs::metadata(&file).unwrap();
        let secs = mtime_unix(Some(&meta)).expect("常规文件应能取到 mtime");
        let now = chrono::Utc::now().timestamp();
        assert!(
            secs > now - 60 && secs <= now + 60,
            "mtime 应接近当前时间：{secs} vs {now}"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn node_文件计数() {
        let dir = std::env::temp_dir().join(format!("zbar-node-count-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("sub/deep")).unwrap();
        fs::write(dir.join("a.node"), b"x").unwrap();
        fs::write(dir.join("sub/b.node"), b"x").unwrap();
        fs::write(dir.join("sub/deep/c.node"), b"x").unwrap();
        fs::write(dir.join("sub/plain.txt"), b"x").unwrap();
        assert_eq!(count_node_files(&dir), 3);
        assert_eq!(count_node_files(&dir.join("not-exist")), 0);
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn plist_版本解析() {
        let dir = std::env::temp_dir().join(format!("zbar-plist-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        let plist = dir.join("Info.plist");
        fs::write(
            &plist,
            r#"<?xml version="1.0"?>
<plist><dict>
    <key>CFBundleIdentifier</key><string>dev.zcode.app</string>
    <key>CFBundleShortVersionString</key>
    <string>2.14.3</string>
</dict></plist>"#,
        )
        .unwrap();
        assert_eq!(parse_plist_version(&plist).as_deref(), Some("2.14.3"));
        // 无版本键 → None
        fs::write(&plist, "<plist><dict><key>A</key><string>b</string></dict></plist>").unwrap();
        assert!(parse_plist_version(&plist).is_none());
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn app_id_白名单校验_拒绝路径遍历() {
        assert!(validate_app_id("zcode").is_ok());
        // 路径遍历与未知值一律拒绝，返回中文错误
        for bad in ["../x", "zcode/../../evil", "", "unknown-app"] {
            let err = validate_app_id(bad).unwrap_err();
            assert!(err.contains("未知应用"), "app_id={bad:?} 的错误应含中文提示：{err}");
        }
    }

    #[test]
    fn 备份守卫_重装场景不覆写既有备份() {
        let dir = std::env::temp_dir().join(format!("zbar-backup-guard-reinstall-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 当前 asar：已被注入主题（内容为注入版）
        let asar = dir.join("app.asar");
        fs::write(&asar, "injected-asar").unwrap();
        // 既有备份：真原版 + 对应 meta
        let backup_dir = dir.join("backup");
        fs::create_dir_all(&backup_dir).unwrap();
        let orig_backup = backup_dir.join(store::backup_file_name(Some("1.0.0"), 100));
        fs::write(&orig_backup, "pristine-original").unwrap();
        store::write_backup_meta_in(&backup_dir, 100, Some("1.0.0".into())).unwrap();

        // 已注入（current_injected=true）→ 跳过备份与 meta 覆写
        let size = fs::metadata(&asar).unwrap().len();
        let did = write_backup_if_pristine(&asar, &backup_dir, Some("1.0.0"), size, true)
            .expect("重装守卫应成功跳过备份");
        assert!(!did, "当前 asar 已注入时应跳过备份");

        // 既有备份文件与 meta 均未被覆写（真原版得以保留）
        assert_eq!(
            fs::read_to_string(&orig_backup).unwrap(),
            "pristine-original",
            "既有备份内容不应被注入版覆盖"
        );
        let meta = store::load_backup_meta_in(&backup_dir).expect("meta.json 应保留原记录");
        assert_eq!(meta.asar_size, 100);
        assert_eq!(meta.zcode_version.as_deref(), Some("1.0.0"));
        // 备份目录不应新增注入版备份
        let baks: Vec<_> = fs::read_dir(&backup_dir)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().ends_with(".bak"))
            .collect();
        assert_eq!(baks.len(), 1, "不应新增备份文件");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 备份守卫_未注入时执行备份并写meta() {
        let dir = std::env::temp_dir().join(format!("zbar-backup-guard-pristine-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 当前 asar：未注入的原版（首次安装 / 应用升级覆盖后的新原版）
        let asar = dir.join("app.asar");
        fs::write(&asar, "pristine-fresh").unwrap();
        let backup_dir = dir.join("backup");
        let size = fs::metadata(&asar).unwrap().len();

        let did = write_backup_if_pristine(&asar, &backup_dir, Some("2.0.0"), size, false)
            .expect("未注入时应执行备份");
        assert!(did, "未注入时不应跳过备份");

        // 备份文件与 meta 均按当前 asar 写入
        let backup_path = backup_dir.join(store::backup_file_name(Some("2.0.0"), size));
        assert_eq!(fs::read_to_string(&backup_path).unwrap(), "pristine-fresh");
        let meta = store::load_backup_meta_in(&backup_dir).expect("meta.json 应写入");
        assert_eq!(meta.asar_size, size);
        assert_eq!(meta.zcode_version.as_deref(), Some("2.0.0"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 备份守卫_备份成功后清理旧版本备份() {
        let dir = std::env::temp_dir().join(format!("zbar-backup-cleanup-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 真机事故形态：backup 目录残留上一版本的旧 .bak（v3.9.2），
        // 本次备份 v3.10.0 成功后旧版本必须被清理（还原永远只应使用
        // meta 指向的最新备份，旧版本每个约 300MB 纯占空间）。
        let asar = dir.join("app.asar");
        fs::write(&asar, "pristine-v310").unwrap();
        let backup_dir = dir.join("backup");
        fs::create_dir_all(&backup_dir).unwrap();
        let stale = backup_dir.join(store::backup_file_name(Some("3.9.2"), 307008658));
        fs::write(&stale, "stale-v392").unwrap();

        let size = fs::metadata(&asar).unwrap().len();
        let did = write_backup_if_pristine(&asar, &backup_dir, Some("3.10.0"), size, false)
            .expect("备份应成功执行");
        assert!(did);

        // 当前备份保留且 meta 指向它；旧版本 .bak 被删除
        let current = backup_dir.join(store::backup_file_name(Some("3.10.0"), size));
        assert_eq!(fs::read_to_string(&current).unwrap(), "pristine-v310");
        assert!(!stale.exists(), "旧版本备份应在备份成功后被清理");
        let meta = store::load_backup_meta_in(&backup_dir).expect("meta.json 应写入");
        assert_eq!(meta.zcode_version.as_deref(), Some("3.10.0"));
        // meta 驱动的选择恰好指向唯一保留的那份（回归闭环）
        assert_eq!(store::latest_backup_in(&backup_dir), Some(current));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 备份守卫_重装路径不清理既有备份() {
        let dir = std::env::temp_dir().join(format!("zbar-backup-no-clean-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 重装守卫提前返回（current_injected=true），不得触碰备份目录：
        // 此刻既有备份是唯一真原版，任何清理/覆写都会造成不可逆丢失
        let asar = dir.join("app.asar");
        fs::write(&asar, "injected-asar").unwrap();
        let backup_dir = dir.join("backup");
        fs::create_dir_all(&backup_dir).unwrap();
        let backup = backup_dir.join(store::backup_file_name(Some("1.0.0"), 100));
        fs::write(&backup, "pristine-original").unwrap();
        let extra = backup_dir.join(store::backup_file_name(Some("0.9.0"), 90));
        fs::write(&extra, "older-extra").unwrap();

        let size = fs::metadata(&asar).unwrap().len();
        let did = write_backup_if_pristine(&asar, &backup_dir, Some("1.0.0"), size, true)
            .expect("重装守卫应成功跳过备份");
        assert!(!did, "当前 asar 已注入时应跳过备份");

        assert!(backup.is_file(), "既有备份不应被清理");
        assert!(extra.is_file(), "重装路径不应触发旧版本清理");
        // 重装路径不写 meta.json（沿用既有记录），目录内不应多出新文件
        assert_eq!(fs::read_dir(&backup_dir).unwrap().flatten().count(), 2);

        let _ = fs::remove_dir_all(&dir);
    }

    /// 回归守护：staging 目录必须放在系统临时目录下、用 zbar-theme- 前缀的
    /// 可见名（与 staging_dir 同一命名规则）。asar pack 的 unpack glob 按完整
    /// 路径匹配，任何点开头的路径段（目录名或祖先）都会导致 "**/*.node"
    /// 匹配失败，原生模块不被拆到 unpacked，安装报"原生模块数量不一致"。
    /// 实际调用 npx，无 node 环境时跳过。
    #[test]
    fn 打包_可见名staging目录_原生模块被拆到unpacked() {
        if !asar::node_available() {
            return;
        }
        let base =
            std::env::temp_dir().join(format!("zbar-staging-regression-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();

        // 与 staging_dir 相同的命名规则（zbar-theme-staging- 前缀路径，
        // 祖先为系统临时目录，无点开头的隐藏段）
        let staging = base.join(staging_dir_name("zcode", 123));
        fs::create_dir_all(staging.join("sub")).unwrap();
        fs::write(staging.join("sub/xxx.node"), b"native").unwrap();

        let packed = base.join(pack_tmp_name("zcode", 123));
        asar::asar_pack_with_unpack(&staging, &packed, "**/*.node")
            .expect("打包自造 staging 目录应成功");

        let unpacked = asar_unpacked_of(&packed);
        assert!(
            unpacked.join("sub/xxx.node").is_file(),
            "原生模块应被拆到 {}",
            unpacked.display()
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// cleanup_staging 应同时清理新旧两代前缀的残留：新版 zbar-theme- 可见名
    /// （系统临时目录下使用）与旧版历史残留（zbar- 可见名、点开头隐藏名，
    /// 位于 ~/.zbar/agent-themes），且不误删无关文件与其它应用的临时目录。
    #[test]
    fn 清理staging_新旧前缀均被清除() {
        let dir = std::env::temp_dir().join(format!("zbar-cleanup-staging-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();

        // 新前缀（temp_dir 位置）：目录 + 临时 asar 文件
        fs::create_dir_all(dir.join("zbar-theme-staging-zcode-1/sub")).unwrap();
        fs::write(dir.join("zbar-theme-staging-zcode-1/sub/x.node"), b"x").unwrap();
        fs::write(dir.join("zbar-theme-pack-zcode-1.asar"), b"x").unwrap();
        // 旧前缀（~/.zbar 历史残留）：zbar- 可见名 + 点开头隐藏名
        fs::create_dir_all(dir.join("zbar-staging-zcode-2")).unwrap();
        fs::write(dir.join("zbar-pack-zcode-2.asar"), b"x").unwrap();
        fs::create_dir_all(dir.join(".staging-zcode-3")).unwrap();
        fs::write(dir.join(".pack-zcode-3.asar"), b"x").unwrap();
        // 不应误删：无关文件、其它应用前缀
        fs::write(dir.join("keep.txt"), b"x").unwrap();
        fs::create_dir_all(dir.join("zbar-theme-staging-other-1")).unwrap();

        // 与 cleanup_staging 相同的两段式调用：先新前缀、后旧前缀
        cleanup_staging_in(
            &dir,
            &[
                "zbar-theme-staging-zcode-".to_string(),
                "zbar-theme-pack-zcode-".to_string(),
            ],
        )
        .expect("清理新前缀应成功");
        cleanup_staging_in(
            &dir,
            &[
                "zbar-staging-zcode-".to_string(),
                "zbar-pack-zcode-".to_string(),
                ".staging-zcode-".to_string(),
                ".pack-zcode-".to_string(),
            ],
        )
        .expect("清理旧前缀应成功");

        assert!(dir.join("keep.txt").is_file(), "无关文件不应被删除");
        assert!(
            dir.join("zbar-theme-staging-other-1").is_dir(),
            "其它应用前缀不应被误删"
        );
        assert!(
            !dir.join("zbar-theme-staging-zcode-1").exists(),
            "新前缀 staging 目录应被清理"
        );
        assert!(
            !dir.join("zbar-theme-pack-zcode-1.asar").exists(),
            "新前缀 pack 文件应被清理"
        );
        assert!(
            !dir.join("zbar-staging-zcode-2").exists(),
            "旧前缀可见名 staging 目录应被清理"
        );
        assert!(
            !dir.join("zbar-pack-zcode-2.asar").exists(),
            "旧前缀可见名 pack 文件应被清理"
        );
        assert!(
            !dir.join(".staging-zcode-3").exists(),
            "旧前缀隐藏 staging 目录应被清理"
        );
        assert!(
            !dir.join(".pack-zcode-3.asar").exists(),
            "旧前缀隐藏 pack 文件应被清理"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    // ============================================================
    // 动态外置清单与 unpacked 集合校验（架构不变量 2/3 的测试）
    // ============================================================

    /// glob 构造纯函数（build_unpack_glob）：
    /// - 空清单 → 退化为 `**/*.node`；
    /// - 常规清单 → 去重 + 排序，`**/*.node` 兜底固定首位、每项带 `**/` 前缀；
    /// - 特殊字符 → glob 元字符转义为字面匹配，`/` 分隔符保持原样。
    #[test]
    fn unpack_glob_构造() {
        // 空清单（官方无 unpacked 目录或目录为空）→ 退化为兜底模式
        assert_eq!(build_unpack_glob(&[]), "**/*.node");
        // 常规清单：重复项去重、排序稳定，兜底固定第一
        let glob = build_unpack_glob(&[
            "node_modules/foo/bar.node".to_string(),
            "icudtl.dat".to_string(),
            "node_modules/foo/bar.node".to_string(), // 重复项
        ]);
        assert_eq!(
            glob,
            "{**/*.node,**/icudtl.dat,**/node_modules/foo/bar.node}",
            "应去重排序且固定含兜底：{glob}"
        );
        // 特殊字符：@scope 命名空间、空格括号、方括号均转义为字面匹配
        let glob = build_unpack_glob(&[
            "node_modules/@scope/pkg/x.node".to_string(),
            "special (1)/y[w].dat".to_string(),
        ]);
        assert!(
            glob.contains("**/node_modules/\\@scope/pkg/x.node"),
            "@ 应被转义：{glob}"
        );
        assert!(
            glob.contains("**/special \\(1\\)/y\\[w\\].dat"),
            "括号与方括号应被转义：{glob}"
        );
        assert!(!glob.contains("\\/"), "不得转义路径分隔符：{glob}");
    }

    /// 相对路径清单收集（collect_rel_files）：递归子目录、隐藏文件/目录
    /// 一并收集（不判断类型）；目录不存在返回空清单。
    #[test]
    fn 相对清单收集_递归与隐藏文件() {
        let dir = std::env::temp_dir().join(format!("zbar-rel-files-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(dir.join("a/b")).unwrap();
        fs::create_dir_all(dir.join(".hidden")).unwrap();
        fs::write(dir.join("top.node"), b"x").unwrap();
        fs::write(dir.join("a/b/deep.dat"), b"x").unwrap();
        fs::write(dir.join(".hidden/dot.node"), b"x").unwrap();

        let mut rels = collect_rel_files(&dir);
        rels.sort();
        assert_eq!(
            rels,
            vec![
                ".hidden/dot.node".to_string(),
                "a/b/deep.dat".to_string(),
                "top.node".to_string(),
            ],
            "应递归收集全部常规文件（含隐藏段）"
        );
        // 目录不存在 → 空清单（官方无 unpacked 目录的退化场景）
        assert!(collect_rel_files(&dir.join("not-exist")).is_empty());

        let _ = fs::remove_dir_all(&dir);
    }

    /// unpacked 集合包含校验（安装⑥，verify_unpacked_superset）：
    /// - 新包拆出 ⊇ 官方现有 → 通过（兜底多拆出的文件不视为错误）；
    /// - 官方有而新包缺 → 报错并中文列出缺失文件；
    /// - 官方目录残缺（本身文件少）→ 只校验官方现有的，不放大缺失；
    /// - 两侧均为空 → 空集合包含空集合，通过。
    #[test]
    fn unpacked_集合包含校验() {
        let base = std::env::temp_dir().join(format!("zbar-superset-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        // 官方目录两个文件；新包目录三个文件（官方两个 + 兜底多拆一个）
        let orig = base.join("orig.unpacked");
        let full = base.join("full.unpacked");
        for d in [&orig, &full] {
            fs::create_dir_all(d.join("native")).unwrap();
            fs::write(d.join("native/a.node"), b"x").unwrap();
            fs::write(d.join("b.dat"), b"x").unwrap();
        }
        fs::write(full.join("native/extra.node"), b"x").unwrap();
        verify_unpacked_superset(&orig, &full).expect("覆盖官方现有文件应通过（多拆不报错）");

        // 官方残缺场景：官方只剩 1 个文件，新包仍覆盖 → 通过（不放大缺失）
        let partial = base.join("partial.unpacked");
        fs::create_dir_all(partial.join("native")).unwrap();
        fs::write(partial.join("native/a.node"), b"x").unwrap();
        verify_unpacked_superset(&partial, &full).expect("官方残缺时只校验现有文件");

        // 缺失场景：新包比官方少文件 → 报错并列出缺失文件
        let err =
            verify_unpacked_superset(&orig, &partial).expect_err("新包缺官方现有文件应报错");
        assert!(err.contains("外置文件不完整"), "错误应说明校验失败：{err}");
        assert!(err.contains("b.dat"), "错误应列出缺失文件：{err}");
        assert!(err.contains("已中止安装"), "错误应说明已中止：{err}");

        // 空目录 / 目录不存在
        let empty = base.join("empty.unpacked");
        fs::create_dir_all(&empty).unwrap();
        verify_unpacked_superset(&empty, &empty).expect("两侧均空应通过");
        verify_unpacked_superset(&orig, &empty)
            .expect_err("官方有文件而新包目录为空应报错");

        let _ = fs::remove_dir_all(&base);
    }

    /// 端到端：由 fake 官方 unpacked 目录动态收集清单构造 glob 打包 staging，
    /// 特殊字符路径（@scope 命名空间、空格括号、方括号）与常规路径全部拆到
    /// 产物 unpacked——实测 minimatch brace + 转义行为（架构不变量 3 的落点），
    /// 并走一遍安装⑥的集合包含校验。实际调用 npx，无 node 环境时跳过。
    #[test]
    fn 打包_动态清单glob_特殊字符路径全部拆出() {
        if !asar::node_available() {
            return;
        }
        let base = std::env::temp_dir().join(format!("zbar-dynamic-glob-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        // staging（解包产物的模拟）：官方外置清单内的文件 + 应留在包内的文件。
        // 路径规则同 staging_dir（祖先不得有点开头段，glob 才能匹配）
        let staging = base.join(staging_dir_name("zcode", 789));
        fs::create_dir_all(staging.join("node_modules/@scope/pkg")).unwrap();
        fs::create_dir_all(staging.join("special (1)")).unwrap();
        fs::create_dir_all(staging.join("out/renderer")).unwrap();
        fs::write(staging.join("node_modules/@scope/pkg/x.node"), b"native").unwrap();
        fs::write(staging.join("special (1)/y[w].dat"), b"data").unwrap();
        fs::write(staging.join("out/renderer/index.html"), "<html></html>").unwrap();

        // fake 官方 unpacked 目录：现状与 staging 内外置文件一一对应
        let official = base.join("app.asar.unpacked");
        fs::create_dir_all(official.join("node_modules/@scope/pkg")).unwrap();
        fs::create_dir_all(official.join("special (1)")).unwrap();
        fs::write(official.join("node_modules/@scope/pkg/x.node"), b"native").unwrap();
        fs::write(official.join("special (1)/y[w].dat"), b"data").unwrap();

        let rels = collect_rel_files(&official);
        let glob = build_unpack_glob(&rels);
        let packed = base.join(pack_tmp_name("zcode", 789));
        asar::asar_pack_with_unpack(&staging, &packed, &glob)
            .expect("动态清单 glob 打包应成功");

        // 产物 unpacked：官方外置清单全部拆出；普通 html 仍在包内不外置
        let new_unpacked = asar_unpacked_of(&packed);
        assert!(
            new_unpacked.join("node_modules/@scope/pkg/x.node").is_file(),
            "@scope 路径应被拆出到 {}（glob：{glob}）",
            new_unpacked.display()
        );
        assert!(
            new_unpacked.join("special (1)/y[w].dat").is_file(),
            "空格/括号路径应被拆出（glob：{glob}）"
        );
        assert!(
            !new_unpacked.join("out/renderer/index.html").exists(),
            "非清单内文件不应外置"
        );
        // 安装⑥的集合包含校验应通过
        verify_unpacked_superset(&official, &new_unpacked)
            .expect("动态清单拆出应覆盖官方现状");

        let _ = fs::remove_dir_all(&base);
    }

    /// 行为记录测试（固化已知行为，非期望行为）：asar pack 的 --unpack glob
    /// 按含全部祖先段的完整路径匹配且不匹配点开头段——只要 staging 路径中
    /// 存在任何点开头的祖先目录（如历史上的 ~/.zbar/agent-themes），原生模块
    /// 就不会被拆到 unpacked，安装报"原生模块数量不一致"。本测试在点开头
    /// 祖先目录下复现该现象并固化，防止未来有人把临时目录挪回隐藏路径而
    /// 测试察觉不到。实际调用 npx，无 node 环境时跳过。
    #[test]
    fn 打包_点开头祖先目录_原生模块不被拆出_行为记录() {
        if !asar::node_available() {
            return;
        }
        // 模拟历史踩坑位置：temp_dir 下人为构造点开头的隐藏祖先段
        let base =
            std::env::temp_dir().join(format!(".zbar-test-glob-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let staging = base.join(staging_dir_name("zcode", 456));
        fs::create_dir_all(staging.join("sub")).unwrap();
        fs::write(staging.join("sub/xxx.node"), b"native").unwrap();

        let packed = base.join(pack_tmp_name("zcode", 456));
        asar::asar_pack_with_unpack(&staging, &packed, "**/*.node")
            .expect("打包本身应成功（glob 问题不影响退出码）");

        let unpacked = asar_unpacked_of(&packed);
        assert_eq!(
            count_node_files(&unpacked),
            0,
            "点开头祖先段下 unpack glob 不匹配，.node 不应被拆到 {}",
            unpacked.display()
        );

        let _ = fs::remove_dir_all(&base);
    }

    /// 清单比对（安装 ⑥ 的核心校验）：同一目录重打包应通过；删过文件的
    /// 目录重打包应失败，且错误中给出差异路径。实际调用 npx，无 node 环境
    /// 时跳过。
    #[test]
    fn 清单比对_一致通过_缺失报差异路径() {
        if !asar::node_available() {
            return;
        }
        let base = std::env::temp_dir().join(format!("zbar-manifest-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(base.join("src/out/renderer")).unwrap();
        fs::write(base.join("src/out/renderer/index.html"), "<html></html>").unwrap();
        fs::write(base.join("src/out/main.js"), "console.log(1)").unwrap();
        fs::create_dir_all(base.join("src/sub")).unwrap();
        fs::write(base.join("src/sub/xxx.node"), b"native").unwrap();

        // 原版：完整目录打包后 list 出清单基线
        let orig_asar = base.join("orig.asar");
        asar::asar_pack_with_unpack(&base.join("src"), &orig_asar, "**/*.node")
            .expect("打包原版应成功");
        let orig_list = asar::asar_list(&orig_asar).expect("list 原版应成功");

        // 场景一：同一目录原样重打包 → 清单一致，校验通过
        let repacked = base.join("repacked.asar");
        asar::asar_pack_with_unpack(&base.join("src"), &repacked, "**/*.node")
            .expect("重打包应成功");
        verify_manifest(&orig_list, &repacked).expect("同目录重打包清单应一致");

        // 场景二：删掉文件后重打包 → 校验失败且错误给出缺失路径
        fs::remove_file(base.join("src/out/main.js")).unwrap();
        let missing_asar = base.join("missing.asar");
        asar::asar_pack_with_unpack(&base.join("src"), &missing_asar, "**/*.node")
            .expect("删文件后打包应成功");
        let err =
            verify_manifest(&orig_list, &missing_asar).expect_err("缺文件清单应校验失败");
        assert!(err.contains("不一致"), "错误应说明清单不一致：{err}");
        assert!(err.contains("out/main.js"), "错误应列出缺失路径：{err}");

        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn 提权暂存名_隐藏名且与asar同目录() {
        let asar = Path::new("/Applications/ZCode.app/Contents/Resources/app.asar");
        assert_eq!(
            asar_staging_sibling(asar, ".incoming"),
            PathBuf::from("/Applications/ZCode.app/Contents/Resources/.zbar-app.asar.incoming")
        );
        assert_eq!(
            asar_staging_sibling(asar, ".rollback"),
            PathBuf::from("/Applications/ZCode.app/Contents/Resources/.zbar-app.asar.rollback")
        );
        // 文件名异常时兜底到系统临时目录，不 panic
        assert_eq!(
            asar_staging_sibling(Path::new("/"), ".incoming"),
            PathBuf::from("/tmp/.zbar-app.asar.incoming")
        );
    }

    /// 单会话替换脚本的顺序与安全不变量：
    /// - set -e 前置、trap 清理临时文件；
    /// - 原始 asar 全脚本只在最后的 mv 目标出现一次（任何前置失败都不触碰）；
    /// - 顺序：cp 临时文件 → xattr 清属性 → 签名 → 校验 → mv 最后原子换入；
    /// - 不含任何 unpacked 操作与删除语义命令（架构不变量 2 的脚本侧守护：
    ///   无 rsync / 无 rm -rf；trap 清理 incoming 的 rm -f 为唯一例外）。
    #[test]
    #[cfg(not(windows))]
    fn 替换脚本_单会话原子换入顺序() {
        let asar = Path::new("/Applications/ZCode.app/Contents/Resources/app.asar");
        let script = build_replace_script(
            Path::new("/tmp/zbar-theme-pack-zcode-1.asar"),
            asar,
            Path::new("/Applications/ZCode.app"),
        );

        // set -e 前置 + 临时文件名 + trap 统一清理
        assert!(script.starts_with("set -e"), "脚本应以 set -e 开头：{script}");
        assert!(script.contains(".zbar-app.asar.incoming"));
        assert!(script.contains("trap"), "应含临时文件清理 trap：{script}");

        // 原始 asar（完整引号包裹）只在 mv 目标出现一次——此前任何步骤失败
        // （含「应用管理」拦截 cp 临时文件）都不会触碰原 asar
        let asar_q = privilege::sh_quote(asar);
        assert_eq!(
            script.matches(&asar_q).count(),
            1,
            "原 asar 应只出现一次（最后的 mv 目标）：{script}"
        );

        // 顺序特征：cp 临时文件 → xattr 清属性 → 签名 → 校验 → mv 最后换入
        let cp_pos = script.find("cp -f").expect("应含 cp");
        let xattr_pos = script.find("xattr -cr").expect("应含 xattr 清属性");
        let sign_pos = script.find("codesign --force --deep --sign -").expect("应含重签名");
        let verify_pos = script.find("codesign --verify --deep --strict").expect("应含签名校验");
        let mv_pos = script.rfind("mv -f").expect("应含 mv 换入");
        assert!(cp_pos < xattr_pos, "cp 应先于 xattr：{script}");
        assert!(xattr_pos < sign_pos, "xattr 应先于签名：{script}");
        assert!(sign_pos < verify_pos, "签名应先于校验：{script}");
        assert!(verify_pos < mv_pos, "校验应先于 mv 换入：{script}");
        assert!(
            script.trim_end().ends_with(&asar_q),
            "mv 换入应为最后一步：{script}"
        );

        // 无裸换行（run_as_admin 的单行约束）
        assert!(!script.contains('\n'), "脚本必须为单行：{script}");
    }

    /// 替换脚本的删除语义守护（rsync --delete 事故的回归测试）：
    /// - 不得出现 rsync（旧版曾用 rsync --delete 同步 unpacked，误删官方
    ///   spawn-helper 等外置文件，重装必败、终端功能受损）；
    /// - 不得出现 rm -rf（trap 清理 incoming 临时文件的 rm -f 为唯一例外，
    ///   且仅允许出现在 trap 内）；
    /// - 不引用官方 unpacked 目录（app.asar.unpacked 永不被触碰）。
    #[test]
    #[cfg(not(windows))]
    fn 替换脚本_不含删除语义命令与unpacked操作() {
        let asar = Path::new("/Applications/ZCode.app/Contents/Resources/app.asar");
        let script = build_replace_script(
            Path::new("/tmp/zbar-theme-pack-zcode-1.asar"),
            asar,
            Path::new("/Applications/ZCode.app"),
        );
        assert!(!script.contains("rsync"), "替换脚本不得含 rsync：{script}");
        assert!(!script.contains("rm -rf"), "替换脚本不得含 rm -rf：{script}");
        // rm 仅允许出现在 trap 清理（单文件 rm -f），不得有其它删除命令
        let rm_count = script.matches("rm ").count();
        let trap_rm = script.matches("rm -f").count();
        assert_eq!(
            rm_count, trap_rm,
            "rm 应仅以 trap 内 rm -f 形式出现：{script}"
        );
        assert!(
            script.contains("trap \"rm -f"),
            "rm -f 应仅存在于 trap 清理中：{script}"
        );
        assert!(
            !script.contains(".unpacked"),
            "替换脚本不得引用 unpacked 目录：{script}"
        );
    }

    /// 单会话还原脚本的不变量：
    /// - 第一步把备份 cp 到临时名（TCC 拦截时原 asar 未动）；
    /// - 原 asar 首次出现晚于备份临时文件落盘（先落盘后挪开再换入）；
    /// - "先验证现有签名、失败才重签"的决策顺序；
    /// - 含还原前 asar 的 rollback 暂存与失败自愈换回。
    #[test]
    #[cfg(not(windows))]
    fn 还原脚本_单会话含验证决策与自愈换回() {
        let asar = Path::new("/Applications/ZCode.app/Contents/Resources/app.asar");
        let backup = Path::new("/Users/u/.zbar/agent-themes/zcode/backup/app.asar.v1.0.0.100.bak");
        let script = build_restore_script(backup, asar, Path::new("/Applications/ZCode.app"));

        assert!(script.starts_with("set -e"), "脚本应以 set -e 开头：{script}");
        assert!(!script.contains('\n'), "脚本必须为单行：{script}");
        // 架构不变量 2：还原脚本同样不得含任何 unpacked 操作与删除语义命令
        assert!(
            !script.contains(".unpacked"),
            "还原脚本不得引用 unpacked 目录：{script}"
        );
        assert!(!script.contains("rsync"), "还原脚本不得含 rsync：{script}");
        assert!(!script.contains("rm -rf"), "还原脚本不得含 rm -rf：{script}");

        // 备份先 cp 到临时名（拦截发生时零损伤）：cp 的源是备份文件，
        // 目标是 .zbar-app.asar.incoming 临时名（而非直接覆盖原 asar）
        let backup_cp = format!("cp -f {} ", privilege::sh_quote(backup));
        let backup_cp_pos = script
            .find(&backup_cp)
            .unwrap_or_else(|| panic!("备份应先 cp 到临时名：{script}"));
        let incoming_q = privilege::sh_quote(asar_staging_sibling(asar, ".incoming"));
        assert!(
            script[backup_cp_pos..].starts_with(&format!("{backup_cp}{incoming_q}")),
            "cp 目标应为临时名而非原 asar：{script}"
        );
        let asar_q = privilege::sh_quote(asar);
        let first_asar_pos = script.find(&asar_q).expect("原 asar 应出现");
        assert!(
            first_asar_pos > backup_cp_pos,
            "原 asar 首次出现应晚于备份临时文件落盘：{script}"
        );

        // 先 verify 后签的决策（失败才 ad-hoc 重签）
        let verify_pos = script.find("codesign --verify").expect("应含签名验证");
        let sign_pos = script.find("codesign --force").expect("应含 ad-hoc 重签");
        assert!(verify_pos < sign_pos, "应先验证现有签名再决定重签：{script}");

        // 还原前 asar 的回滚暂存 + 失败自愈换回
        assert!(script.contains(".zbar-app.asar.rollback"), "应含回滚暂存名：{script}");
        let rollback_pos = script.find(".zbar-app.asar.rollback").unwrap();
        let heal_mv_pos = script
            .rfind("mv -f")
            .expect("应含自愈换回的 mv");
        assert!(rollback_pos < heal_mv_pos);
        // 换回后 exit 1（整脚本非零退出，Rust 侧据此报错）
        assert!(script.contains("exit 1"), "自愈换回后应显式失败退出：{script}");
    }

    // ============================================================
    // 三级策略执行器（探针 / 直写）测试
    // ============================================================

    #[test]
    fn 可写探针_errno分类() {
        use std::io::Error;
        // EPERM(1)：macOS「应用管理」TCC 拦截特征
        assert_eq!(
            classify_write_error(&Error::from_raw_os_error(1)),
            WriteProbe::BlockedByTcc,
            "EPERM 应判为 TCC 拦截"
        );
        // EACCES(13) / EROFS(30) / ENOENT(2)：普通权限不足类，走提权兜底
        for code in [13, 30, 2] {
            assert_eq!(
                classify_write_error(&Error::from_raw_os_error(code)),
                WriteProbe::NeedPrivilege,
                "errno {code} 应归入提权兜底"
            );
        }
        // 非 unix / 无 errno 场景（自定义错误）：同样走兜底
        assert_eq!(
            classify_write_error(&Error::other("boom")),
            WriteProbe::NeedPrivilege
        );
    }

    #[test]
    fn 可写探针_临时目录可写_不存在目录走兜底() {
        let dir = std::env::temp_dir().join(format!("zbar-probe-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        // 可写目录：探针成功且不留残留文件
        assert_eq!(probe_writable(&dir), WriteProbe::Writable);
        assert!(
            !dir.join(".zbar-write-probe").exists(),
            "探针文件应随即删除"
        );
        // 不存在目录：ENOENT 归入提权兜底类（真实错误由兜底脚本报出）
        assert_eq!(
            probe_writable(&dir.join("not-exist")),
            WriteProbe::NeedPrivilege
        );
        let _ = fs::remove_dir_all(&dir);
    }

    /// 只读目录（EACCES）必须归入"提权兜底"而非误判为 TCC 拦截。
    /// 以 root 运行测试时 mode 位不生效（目录仍可写），该情况下
    /// 允许 Writable 结论，但绝不允许出现 BlockedByTcc 误判。
    #[test]
    #[cfg(unix)]
    fn 可写探针_只读目录不误判为tcc拦截() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("zbar-probe-ro-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o555)).unwrap();
        let result = probe_writable(&dir);
        // 先恢复权限再断言/清理，避免断言失败残留只读目录
        fs::set_permissions(&dir, fs::Permissions::from_mode(0o755)).unwrap();
        let _ = fs::remove_dir_all(&dir);
        assert_ne!(
            result,
            WriteProbe::BlockedByTcc,
            "EACCES（权限不足）不应误判为「应用管理」拦截"
        );
        assert_eq!(
            result,
            WriteProbe::NeedPrivilege,
            "非 root 运行时只读目录应归入提权兜底（root 下实测为 {:?}）",
            result
        );
    }

    /// 直写执行器小端到端：临时目录构造 fake bundle（Contents/Resources），
    /// 用 build_replace_script 的原脚本（cp 临时文件 → xattr → codesign
    /// 重签+校验 → 原子 mv 换入）走 run_script_direct，验证：新 asar 已换入、
    /// incoming 临时文件被消费/清理、**官方 unpacked 目录原封不动**（架构
    /// 不变量 2 的端到端守护：即使 pack 产物旁有新 unpacked 目录，脚本也
    /// 不得向官方目录写入或删除任何文件——旧版 rsync --delete 事故的
    /// 回归测试）。仅 macOS（依赖 codesign）；codesign 不可用的受限环境跳过。
    #[test]
    #[cfg(target_os = "macos")]
    fn 直写执行_临时目录脚本跑通并原子换入() {
        let base =
            std::env::temp_dir().join(format!("zbar-direct-exec-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);

        // fake bundle：最小 .app 结构（Info.plist + Mach-O 可执行 + Resources）
        let bundle = base.join("Fake.app");
        let resources = bundle.join("Contents/Resources");
        fs::create_dir_all(bundle.join("Contents/MacOS")).unwrap();
        fs::create_dir_all(&resources).unwrap();
        fs::write(
            bundle.join("Contents/Info.plist"),
            r#"<?xml version="1.0"?>
<plist version="1.0"><dict>
    <key>CFBundleIdentifier</key><string>test.zbar.fake</string>
    <key>CFBundleExecutable</key><string>run</string>
</dict></plist>"#,
        )
        .unwrap();
        // 主可执行文件用系统 shell 副本（真 Mach-O，codesign 可签）
        fs::copy("/bin/sh", bundle.join("Contents/MacOS/run")).unwrap();

        // 官方 asar + 官方 unpacked（含官方外置文件，替换后必须原样保留）
        let asar = resources.join("app.asar");
        fs::write(&asar, "orig-asar").unwrap();
        let orig_unpacked = resources.join("app.asar.unpacked");
        fs::create_dir_all(orig_unpacked.join("native")).unwrap();
        fs::write(orig_unpacked.join("native/official.node"), b"official").unwrap();
        // pack 产物 + 其旁边的临时 unpacked（脚本不应把它同步进官方目录）
        let packed = base.join("packed.asar");
        fs::write(&packed, "new-asar").unwrap();
        let new_unpacked = base.join("packed.asar.unpacked");
        fs::create_dir_all(new_unpacked.join("native")).unwrap();
        fs::write(new_unpacked.join("native/tmp-only.node"), b"tmp").unwrap();

        // codesign 能力探测：先对 fake bundle 试签一次（与正式脚本同款命令），
        // codesign 缺失或受限环境（CI 沙箱等）签名失败时跳过本测试
        let signable = Command::new("codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(&bundle)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);
        if !signable {
            let _ = fs::remove_dir_all(&base);
            return;
        }

        let script = build_replace_script(&packed, &asar, &bundle);
        run_script_direct(&script)
            .unwrap_or_else(|e| panic!("临时目录直写脚本应执行成功（环境异常请排查）：{e}"));

        // 新 asar 已原子换入；incoming 临时文件被 mv 消费（trap rm -f 幂等清理）
        assert_eq!(fs::read_to_string(&asar).unwrap(), "new-asar");
        assert!(
            !asar_staging_sibling(&asar, ".incoming").exists(),
            "incoming 临时文件应不存在"
        );
        // 官方 unpacked 目录原封不动：官方文件保留、临时 unpacked 的文件
        // 未被写入、官方目录未出现任何增删（不写入、不删除、不同步）
        assert!(
            orig_unpacked.join("native/official.node").is_file(),
            "官方 unpacked 现有文件必须原样保留"
        );
        assert_eq!(
            fs::read(orig_unpacked.join("native/official.node")).unwrap(),
            b"official",
            "官方 unpacked 文件内容不得被改动"
        );
        assert!(
            !orig_unpacked.join("native/tmp-only.node").exists(),
            "临时 unpacked 的文件不得被同步进官方目录"
        );
        assert_eq!(
            collect_rel_files(&orig_unpacked),
            vec!["native/official.node".to_string()],
            "官方 unpacked 目录文件集合应与替换前完全一致"
        );

        let _ = fs::remove_dir_all(&base);
    }
}

// ============================================================
// Windows 专属：原生替换器不变量 / 提权 .cmd 构造
// （安装根目录探测与其纯函数测试已随候选构造函数下沉 accounts；
//   本机 macOS 不运行，随 cargo check --target x86_64-pc-windows-msvc
//   --tests 保证语法与逻辑静态正确；真机验证另行执行）
// ============================================================

#[cfg(all(windows, test))]
mod windows_tests {
    use super::*;

    /// Windows 原生替换器不变量（windows_replace_asar，安装与还原共用）：
    /// - 源缺失 → 报中文错误，原 asar 内容原样（copy 阶段失败零损伤）、
    ///   无 incoming 残留（失败路径清理）；
    /// - 源存在 → 原子换入：asar 内容换新、incoming 被 rename 消费、
    ///   源文件保留（备份场景依赖）。
    #[test]
    fn 原生替换_先copy后rename不中途触碰原asar() {
        let base = std::env::temp_dir().join(format!("zbar-winreplace-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let resources = base.join("resources");
        fs::create_dir_all(&resources).unwrap();
        let asar = resources.join("app.asar");
        fs::write(&asar, "orig-asar").unwrap();

        // 源缺失：copy 失败 → 原 asar 未被触碰 + incoming 清理
        let missing = base.join("packed-missing.asar");
        let err = windows_replace_asar(&missing, &asar).unwrap_err();
        assert!(
            err.contains("写入临时替换文件失败"),
            "错误应为中文且说明阶段：{err}"
        );
        assert_eq!(
            fs::read_to_string(&asar).unwrap(),
            "orig-asar",
            "copy 阶段失败不得触碰原 asar"
        );
        assert!(
            !asar_staging_sibling(&asar, ".incoming").exists(),
            "失败路径应清理 incoming 残留"
        );

        // 成功：copy → rename 原子换入（模拟安装换入新包）
        let src = base.join("packed.asar");
        fs::write(&src, "new-asar").unwrap();
        windows_replace_asar(&src, &asar).expect("原生替换应成功");
        assert_eq!(fs::read_to_string(&asar).unwrap(), "new-asar", "asar 应换为新内容");
        assert!(
            !asar_staging_sibling(&asar, ".incoming").exists(),
            "incoming 应被 rename 消费"
        );
        assert_eq!(
            fs::read_to_string(&src).unwrap(),
            "new-asar",
            "源文件应保留（还原场景即备份不被消耗）"
        );

        // 还原同款：备份 → incoming → rename 换回
        let backup = base.join("app.asar.v1.0.0.284.bak");
        fs::write(&backup, "orig-asar").unwrap();
        windows_replace_asar(&backup, &asar).expect("原生还原应成功");
        assert_eq!(fs::read_to_string(&asar).unwrap(), "orig-asar", "asar 应换回备份内容");

        let _ = fs::remove_dir_all(&base);
    }

    /// Windows 提权 .cmd（build_windows_elevate_cmd）的顺序与安全不变量：
    /// - 以 @echo off 开头、含 chcp 65001（UTF-8 内容的中文路径解析）；
    /// - copy 到临时名先于 move 换入（任何前置失败原 asar 未被触碰）；
    /// - move 失败回退 del + move（与 windows_replace_asar 同款语义）；
    /// - 常规路径不引入 %% 转义（% 翻倍仅对含 % 的路径生效）。
    #[test]
    fn 提权cmd脚本_顺序不变量() {
        let asar = Path::new(r"C:\Program Files\ZCode\resources\app.asar");
        let src = Path::new(r"C:\Users\u\AppData\Local\Temp\zbar-theme-pack-zcode-1.asar");
        let cmd = build_windows_elevate_cmd(src, asar);

        assert!(cmd.starts_with("@echo off"), "应以 @echo off 开头：{cmd}");
        assert!(cmd.contains("chcp 65001"), "应切 UTF-8 代码页：{cmd}");
        let copy_pos = cmd.find("copy /Y").expect("应含 copy");
        let move_pos = cmd.find("move /Y").expect("应含 move");
        assert!(copy_pos < move_pos, "copy 应先于 move：{cmd}");
        assert!(cmd.contains("del /f"), "应含失败回退 del：{cmd}");
        let del_pos = cmd.find("del /f").unwrap();
        let fallback_move_pos = cmd.rfind("move /Y").unwrap();
        assert!(del_pos < fallback_move_pos, "回退分支应先 del 后 move：{cmd}");
        // 原 asar 引用仅出现在 move/del 目标（copy 阶段不触碰）
        let asar_q = escape_cmd_path(asar);
        assert_eq!(
            cmd.matches(&asar_q).count(),
            3,
            "原 asar 应仅在 move/del/回退 move 三处出现：{cmd}"
        );
        // 常规路径不含 %，不产生 %% 转义副作用
        assert!(!cmd.contains("%%"), "常规路径不应引入 %%：{cmd}");
        // 不引用 unpacked 目录（架构不变量 2 的脚本侧守护）
        assert!(!cmd.contains(".unpacked"), "不得引用 unpacked 目录：{cmd}");
    }

    /// cmd 路径转义（escape_cmd_path）：双引号包裹 + % 翻倍防变量展开，
    /// 其余字符（含中文、空格）保持字面量。
    #[test]
    fn cmd路径转义_百分号翻倍() {
        assert_eq!(
            escape_cmd_path(Path::new(r"C:\Program Files\ZCode")),
            r#""C:\Program Files\ZCode""#
        );
        assert_eq!(
            escape_cmd_path(Path::new(r"C:\100%\x")),
            r#""C:\100%%\x""#
        );
        assert_eq!(
            escape_cmd_path(Path::new(r"C:\用户 zcode\app")),
            r#""C:\用户 zcode\app""#
        );
    }
}

//! 多智谱账号切换：快照存储 + 切换事务 + ZCode 进程控制。
//!
//! 【重要声明】本模块是整个应用中**唯一**允许写 ZCode 数据目录的位置
//! （默认 `~/.zcode/v2/`，支持 ZCode「更改数据目录」迁移，定位见 zcode_dir），
//! 且只写 `credentials.json` 与 `config.json` 两个文件、只发生在切换事务
//! （switch_account）内部。额度查询（quota.rs）对该目录严格只读，两者互不干扰。
//!
//! 设计要点：
//! - 快照保存在 `~/.zbar/accounts/<id>.account.json`（目录 0700、文件 0600），
//!   其中 credentials 按原文整串保存——键集由 ZCode 客户端自行演进，原文回写
//!   天然兼容未来的键增删；config 只保存 key 含 `coding-plan` 的 provider。
//! - 切换事务顺序 = 先退出桌面应用后写入：ZCode 运行中写凭证文件会被其
//!   内部状态覆盖或破坏登录态，必须先退出（CLI 进程不动，新调用自然读新配置）。
//! - 切换前先把两文件原文备份到 `~/.zbar/accounts/.last/`（点开头不被快照
//!   扫描命中），任何一步失败走回滚，保证零损坏。
//! - 指纹（user_id）解密失败时降级为 unknown-id 快照，不阻塞捕获。

use crate::pricing::config_dir;
use crate::zcode_crypto::{fingerprint_of_credentials, Fingerprint};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
// macOS/Windows 退出进程的轮询等待使用（Linux 下避免 unused import）
#[cfg(any(target_os = "macos", windows))]
use std::time::Duration;

// ============================================================
// 数据结构
// ============================================================

/// 账号快照元信息（列表/捕获返回给前端的形态）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountMeta {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    pub fingerprint: String,
    pub created_at: i64,
    #[serde(default)]
    pub is_current: bool,
}

/// 当前实时登录账号（对 ~/.zcode/v2/credentials.json 解密推断而来）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CurrentAccount {
    pub fingerprint: String,
    pub email: Option<String>,
    /// 当前账号匹配到的快照 id（未捕获过时为 None）
    pub matched_snapshot_id: Option<String>,
}

/// list_accounts 返回：当前登录 + 快照列表。
#[derive(Debug, Serialize, Deserialize)]
pub struct AccountsState {
    pub current: Option<CurrentAccount>,
    pub accounts: Vec<AccountMeta>,
}

/// capture_account 返回。
#[derive(Debug, Serialize)]
pub struct CaptureOutcome {
    pub account: AccountMeta,
    pub updated_existing: bool,
}

/// switch_account 返回。
#[derive(Debug, Serialize)]
pub struct SwitchOutcome {
    pub switched_to: String,
    /// ZCode 桌面应用是否自动重启成功（false 时前端提示手动打开）
    pub zcode_relaunched: bool,
}

/// 磁盘上的快照完整内容（~/.zbar/accounts/<id>.account.json）。
#[derive(Debug, Clone, Serialize, Deserialize)]
struct AccountSnapshot {
    version: i32,
    id: String,
    fingerprint: String,
    display_name: String,
    email: Option<String>,
    created_at: i64,
    updated_at: i64,
    /// credentials.json 原文整串（保留原始格式与全部键，回写零损失）
    credentials_raw: String,
    /// config.json 中 key 含 "coding-plan" 的 provider（含 apiKey 为空的）
    config_providers: Map<String, Value>,
    /// 捕获时按固定顺序选中的 Coding Plan provider key。
    /// 多账号切换后 live config 可能混入其他账号的同前缀 key（切换事务只逐
    /// key 覆盖不清理），额度查询优先按本字段取凭证，保证「快照身份 ↔ 凭证」
    /// 一一对应（老快照无此字段，None 时回退 pick 固定顺序）。
    #[serde(default)]
    login_provider: Option<String>,
    /// 用户是否手动重命名过（true 时重捕获不再覆盖 display_name；
    /// 老快照无此字段，serde 默认 false 即"未锁定"，保持原刷新行为）
    #[serde(default)]
    name_locked: bool,
}

/// 切换前读到的两文件现场（None = 文件不存在）。
#[derive(Debug, Clone, Default)]
struct LiveFiles {
    credentials: Option<String>,
    config: Option<String>,
}

// ============================================================
// 第一节：快照存储（~/.zbar/accounts/）
// ============================================================

/// 全部写操作（捕获/切换/删除/重命名）共用的互斥锁，
/// 防止并发写快照目录与 ~/.zcode 造成交错损坏。
static ACCOUNTS_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

fn accounts_lock() -> &'static Mutex<()> {
    ACCOUNTS_LOCK.get_or_init(|| Mutex::new(()))
}

fn now_ms() -> i64 {
    chrono::Utc::now().timestamp_millis()
}

/// 快照目录（生产路径 ~/.zbar/accounts/）。
fn accounts_dir() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("accounts"))
}

/// 切换前备份目录（~/.zbar/accounts/.last/，点开头不被快照扫描命中）。
fn backup_dir() -> Result<PathBuf, String> {
    Ok(accounts_dir()?.join(".last"))
}

/// ZCode 数据目录与两文件路径。支持 ZCode「更改数据目录」（setting.json 的
/// dataBaseDir，如 D:\app\ZCode-cache）——迁移后凭证写在 {dataBaseDir}/.zcode/v2/，
/// 默认位置只剩旧数据；解析逻辑见 quota::zcode_v2_dir（本应用唯一入口）。
fn zcode_dir() -> Result<PathBuf, String> {
    crate::quota::zcode_v2_dir()
}

fn credentials_path() -> Result<PathBuf, String> {
    Ok(zcode_dir()?.join("credentials.json"))
}

fn zcode_config_path() -> Result<PathBuf, String> {
    Ok(zcode_dir()?.join("config.json"))
}

/// id 只允许 [A-Za-z0-9_-]，防路径穿越。
fn valid_id(id: &str) -> bool {
    !id.is_empty()
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// 由指纹生成快照 id（过滤非法字符；解密失败退化 unknown-<时间戳>）。
fn snapshot_id_of(fp: Option<&Fingerprint>) -> String {
    match fp {
        Some(f) => {
            let filtered: String = f
                .user_id
                .chars()
                .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                .collect();
            if filtered.is_empty() {
                format!("unknown-{}", now_ms())
            } else {
                filtered
            }
        }
        None => format!("unknown-{}", now_ms()),
    }
}

/// 目录权限收紧到 0700（仅 Unix；Windows 走 ACL 不处理）。
fn harden_dir(dir: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(dir, fs::Permissions::from_mode(0o700));
    }
    let _ = dir;
}

/// 文件权限收紧到 0600。
fn harden_file(path: &Path) {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = fs::set_permissions(path, fs::Permissions::from_mode(0o600));
    }
    let _ = path;
}

/// 同目录临时文件 + rename 原子写（快照含凭证，必须避免半截文件）。
fn atomic_write(path: &Path, contents: &str) -> Result<(), String> {
    let dir = path
        .parent()
        .ok_or_else(|| format!("路径缺少父目录: {}", path.display()))?;
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".into());
    let tmp = dir.join(format!(".{name}.tmp"));
    fs::write(&tmp, contents).map_err(|e| format!("写入临时文件失败: {e}"))?;
    harden_file(&tmp);
    fs::rename(&tmp, path).map_err(|e| format!("替换文件失败: {e}"))
}

/// 在指定基目录下保存快照（tmp+rename，目录 0700 / 文件 0600）。
fn save_snapshot_at(base: &Path, snap: &AccountSnapshot) -> Result<(), String> {
    let dir = base.join("accounts");
    fs::create_dir_all(&dir).map_err(|e| format!("创建快照目录失败: {e}"))?;
    harden_dir(&dir);
    let data = serde_json::to_string_pretty(snap)
        .map_err(|e| format!("序列化快照失败: {e}"))?;
    atomic_write(&dir.join(format!("{}.account.json", snap.id)), &data)
}

/// 读取指定基目录下的单个快照（不存在/损坏/非法 id 均返回 None）。
fn load_snapshot_at(base: &Path, id: &str) -> Option<AccountSnapshot> {
    if !valid_id(id) {
        return None;
    }
    let path = base.join("accounts").join(format!("{id}.account.json"));
    let raw = fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// 扫描指定基目录下全部快照完整内容（损坏文件跳过并记日志），按创建时间升序。
fn load_snapshots_at(base: &Path) -> Vec<AccountSnapshot> {
    let dir = base.join("accounts");
    let entries = match fs::read_dir(&dir) {
        Ok(e) => e,
        Err(_) => return vec![], // 目录不存在视为空，不报错
    };
    let mut out: Vec<AccountSnapshot> = vec![];
    for entry in entries.flatten() {
        let path = entry.path();
        let name = path
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        if !name.ends_with(".account.json") {
            continue; // .last/ 在子目录里，天然不会命中
        }
        match fs::read_to_string(&path)
            .map_err(|e| e.to_string())
            .and_then(|s| serde_json::from_str::<AccountSnapshot>(&s).map_err(|e| e.to_string()))
        {
            Ok(s) => out.push(s),
            Err(e) => eprintln!("[zbar-accounts] 跳过损坏的快照文件 {name}: {e}"),
        }
    }
    out.sort_by_key(|s| s.created_at);
    out
}

/// 快照元信息列表（复用 load_snapshots_at，按创建时间升序）。
fn load_meta_list_at(base: &Path) -> Vec<AccountMeta> {
    load_snapshots_at(base)
        .iter()
        .map(AccountMeta::from)
        .collect()
}

/// 删除指定基目录下的快照文件。
fn remove_snapshot_at(base: &Path, id: &str) -> Result<(), String> {
    if !valid_id(id) {
        return Err("非法的账号 id".into());
    }
    let path = base.join("accounts").join(format!("{id}.account.json"));
    if !path.exists() {
        return Err("未找到该账号快照".into());
    }
    fs::remove_file(&path).map_err(|e| format!("删除快照失败: {e}"))
}

impl From<&AccountSnapshot> for AccountMeta {
    fn from(s: &AccountSnapshot) -> Self {
        AccountMeta {
            id: s.id.clone(),
            display_name: s.display_name.clone(),
            email: s.email.clone(),
            fingerprint: s.fingerprint.clone(),
            created_at: s.created_at,
            is_current: false, // 由 list_accounts 按实时指纹回填
        }
    }
}

// ============================================================
// 第二节：读取现场 / 捕获 / 切换事务（含回滚）
// ============================================================

/// 读取当前两文件原文（None = 确认不存在）。存在但读取失败（权限/IO 故障）
/// 必须返回 Err 中止切换：若误当 None 处理，回滚会按"切换前没有该文件"
/// 删除现场登录态。
fn read_live_files() -> Result<LiveFiles, String> {
    let read = |p: Result<std::path::PathBuf, String>, what: &str| -> Result<Option<String>, String> {
        let p = p.map_err(|e| format!("无法定位 {what} 路径（{e}）"))?;
        match fs::read_to_string(&p) {
            Ok(s) => Ok(Some(s)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(format!("读取 {what} 失败: {e}")),
        }
    };
    Ok(LiveFiles {
        credentials: read(credentials_path(), "credentials.json")?,
        config: read(zcode_config_path(), "config.json")?,
    })
}

/// 解析当前 credentials.json 并提取指纹（文件缺失/格式异常/解密失败均 None）。
/// pub(crate)：quota.rs 的 fetch_quota 写历史采样时需要当前账号指纹。
pub(crate) fn current_fingerprint() -> Option<Fingerprint> {
    let raw = fs::read_to_string(credentials_path().ok()?).ok()?;
    let v: Value = serde_json::from_str(&raw).ok()?;
    fingerprint_of_credentials(&v)
}

/// 从 config.json 原文提取 key 含 "coding-plan" 的 provider
/// （含 apiKey 为空的——是否登录由 credentials 决定，provider 定义保留完整）。
/// config 缺失/解析失败返回空 map，捕获不因此失败。
fn coding_plan_providers(config_raw: Option<&str>) -> Map<String, Value> {
    let mut out = Map::new();
    if let Some(raw) = config_raw {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(p) = v.get("provider").and_then(|v| v.as_object()) {
                for (k, val) in p {
                    if k.contains("coding-plan") {
                        out.insert(k.clone(), val.clone());
                    }
                }
            }
        }
    }
    out
}

/// 快照默认显示名：昵称 > 邮箱 > 账号-{id前8}。
fn default_display_name(fp: Option<&Fingerprint>, email: Option<&str>, id: &str) -> String {
    if let Some(name) = fp.and_then(|f| f.display_name.as_deref()) {
        if !name.is_empty() {
            return name.to_string();
        }
    }
    email
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("账号-{}", &id[..id.len().min(8)]))
}

/// 捕获当前登录：读两文件原文 → 提取指纹 → upsert 快照。
/// 重复捕获同一账号时保留原 created_at，只刷新凭证数据；
/// display_name 在旧快照未被手动重命名（name_locked=false）时才刷新。
pub fn capture_account() -> Result<CaptureOutcome, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    let live = read_live_files()?;
    let raw = live.credentials.clone().ok_or(
        "未找到 ZCode 登录凭证（ZCode 数据目录下 credentials.json 不存在），请先在 ZCode 客户端登录后再捕获",
    )?;
    let creds: Value = serde_json::from_str(&raw)
        .map_err(|e| format!("credentials.json 格式异常: {e}"))?;
    let fp = fingerprint_of_credentials(&creds); // 解密失败降级 None，不阻塞
    let id = snapshot_id_of(fp.as_ref());
    let fingerprint = fp.as_ref().map(|f| f.user_id.clone()).unwrap_or_default();
    let email = fp.as_ref().and_then(|f| f.email.clone());
    let providers = coding_plan_providers(live.config.as_deref());
    // 记录捕获时选中的 provider（与主面板 query_quota 同口径），额度查询
    // 优先按它取凭证，避免后续 config 混入其他账号 key 时张冠李戴
    let login_provider =
        crate::quota::pick_coding_plan_api_key(&providers).map(|(k, _, _)| k);

    let now = now_ms();
    let (snapshot, updated_existing) = match load_snapshot_at(&config_dir()?, &id) {
        Some(old) => (
            AccountSnapshot {
                version: 1,
                id: old.id,
                fingerprint, // 重捕获刷新（可能从 unknown 空指纹升级为真实指纹）
                // name_locked=true 表示用户手动重命名过，不能覆盖；否则按默认优先级刷新
                display_name: if old.name_locked {
                    old.display_name
                } else {
                    default_display_name(fp.as_ref(), email.as_deref(), &id)
                },
                email,
                created_at: old.created_at,
                updated_at: now,
                credentials_raw: raw,
                config_providers: providers,
                login_provider,
                name_locked: old.name_locked,
            },
            true,
        ),
        None => {
            let display_name = default_display_name(fp.as_ref(), email.as_deref(), &id);
            (
                AccountSnapshot {
                    version: 1,
                    id,
                    fingerprint,
                    display_name,
                    email,
                    created_at: now,
                    updated_at: now,
                    credentials_raw: raw,
                    config_providers: providers,
                    login_provider,
                    name_locked: false,
                },
                false,
            )
        }
    };
    let account = AccountMeta::from(&snapshot);
    save_snapshot_at(&config_dir()?, &snapshot)?;
    Ok(CaptureOutcome {
        account,
        updated_existing,
    })
}

/// 列出全部快照 + 实时推断当前登录账号。
pub fn list_accounts() -> Result<AccountsState, String> {
    let base = config_dir()?;
    let mut accounts = load_meta_list_at(&base);
    let current = current_fingerprint().map(|fp| {
        let matched = accounts
            .iter()
            .find(|a| a.fingerprint == fp.user_id)
            .map(|a| a.id.clone());
        CurrentAccount {
            fingerprint: fp.user_id,
            email: fp.email,
            matched_snapshot_id: matched,
        }
    });
    // is_current 只按实时指纹与快照指纹的匹配回填
    if let Some(mid) = current.as_ref().and_then(|c| c.matched_snapshot_id.clone()) {
        for a in accounts.iter_mut() {
            a.is_current = a.id == mid;
        }
    }
    Ok(AccountsState { current, accounts })
}

/// 切换到指定快照账号。事务顺序 = 先退出桌面应用后写入；
/// 任一步失败回滚到切换前状态（详见模块头注释）。
/// expect_fingerprint：调用方（无人值守自动切换）在发起时观察到的当前登录
/// 指纹；持锁读出现场后若与之不符，说明等待锁期间登录态已被其他切换改变
/// （如用户手动切换），直接取消，避免把用户刚切好的账号再切走。
pub fn switch_account(
    id: &str,
    expect_fingerprint: Option<&str>,
) -> Result<SwitchOutcome, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());

    // 1. 读目标快照（失败零改动）
    let snapshot =
        load_snapshot_at(&config_dir()?, id).ok_or_else(|| "未找到该账号快照".to_string())?;

    // 2. 记录当前两文件现场（存在但读取失败时中止，零改动）
    let live =
        read_live_files().map_err(|e| format!("读取当前登录态失败（{e}），已取消切换"))?;

    // 2.5 前提校验（零改动）：提供了期望指纹而现场不符（含读不出当前指纹）时
    // 取消。注意错误文案是前端识别"自动切换被取消"的约定，勿改动。
    if let Some(expect) = expect_fingerprint {
        let actual = live.credentials.as_deref().and_then(|raw| {
            serde_json::from_str::<Value>(raw)
                .ok()
                .and_then(|v| fingerprint_of_credentials(&v).map(|fp| fp.user_id))
        });
        if actual.as_deref() != Some(expect) {
            return Err("登录态已变化，已取消本次自动切换".into());
        }
    }

    // 3. 备份到 .last/（失败零改动）
    backup_live(&live).map_err(|e| format!("备份失败（{e}），已取消切换"))?;

    // 4. 当前登录已是目标账号 → 拒绝（零改动）
    if let Some(raw) = &live.credentials {
        if let Ok(v) = serde_json::from_str::<Value>(raw) {
            if let Some(fp) = fingerprint_of_credentials(&v) {
                if fp.user_id == snapshot.fingerprint {
                    return Err("该账号已是当前登录账号".into());
                }
            }
        }
    }

    // 5. 退出 ZCode 桌面应用（未运行直接跳过；失败零改动）
    quit_zcode().map_err(|e| format!("{e}，已取消切换"))?;

    // 6. credentials.json 按原文整串回写
    if let Err(e) = atomic_write(
        &credentials_path()?,
        &snapshot.credentials_raw,
    ) {
        return rollback(&live, format!("写入 credentials.json 失败（{e}）"));
    }

    // 7. config.json 合并写：coding-plan provider 逐 key 覆盖，其余配置保留
    let config_write = match &live.config {
        Some(raw) => match serde_json::from_str::<Value>(raw) {
            Ok(mut v) => {
                let providers = v.get_mut("provider").and_then(|p| p.as_object_mut());
                match providers {
                    Some(p) => {
                        for (k, val) in &snapshot.config_providers {
                            p.insert(k.clone(), val.clone());
                        }
                    }
                    None => {
                        v["provider"] = Value::Object(snapshot.config_providers.clone());
                    }
                }
                serde_json::to_string_pretty(&v)
                    .map_err(|e| format!("序列化 config.json 失败: {e}"))
                    .and_then(|s| atomic_write(&zcode_config_path()?, &s))
            }
            Err(e) => Err(format!("现有 config.json 无法解析: {e}")),
        },
        None => serde_json::to_string_pretty(&serde_json::json!({
            "provider": snapshot.config_providers
        }))
        .map_err(|e| format!("序列化 config.json 失败: {e}"))
        .and_then(|s| atomic_write(&zcode_config_path()?, &s)),
    };
    if let Err(e) = config_write {
        return rollback(&live, format!("写入 config.json 失败（{e}）"));
    }

    // 8. 重读校验：两文件必须都是合法 JSON，否则回滚
    for (name, path) in [
        ("credentials.json", credentials_path()?),
        ("config.json", zcode_config_path()?),
    ] {
        match fs::read_to_string(&path) {
            Ok(raw) if serde_json::from_str::<Value>(&raw).is_ok() => {}
            Ok(_) | Err(_) => {
                return rollback(&live, format!("写入后校验 {name} 失败（内容不是合法 JSON）"));
            }
        }
    }

    // 9. 重启 ZCode（失败不算切换失败，仅提示手动打开）
    let relaunched = launch_zcode();
    Ok(SwitchOutcome {
        switched_to: snapshot.display_name,
        zcode_relaunched: relaunched,
    })
}

/// 切换前备份两文件原文到 ~/.zbar/accounts/.last/（tmp+rename+0600）。
/// 切换前不存在的文件会清掉 .last/ 中的旧备份，保证 .last/ 始终精确反映切换前现场。
fn backup_live(live: &LiveFiles) -> Result<(), String> {
    let dir = backup_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建备份目录失败: {e}"))?;
    harden_dir(&dir);
    match &live.credentials {
        Some(raw) => atomic_write(&dir.join("credentials.json"), raw)?,
        None => {
            let _ = fs::remove_file(dir.join("credentials.json"));
        }
    }
    match &live.config {
        Some(raw) => atomic_write(&dir.join("config.json"), raw)?,
        None => {
            let _ = fs::remove_file(dir.join("config.json"));
        }
    }
    Ok(())
}

/// 回滚：把 .last/ 备份写回 ~/.zcode/v2/（切换前不存在的文件删除目标）。
/// 无论回滚成败都尽力重启 ZCode，让用户能立即看到/处理结果。
fn rollback(live: &LiveFiles, step: String) -> Result<SwitchOutcome, String> {
    let restore = restore_backup(live);
    let _ = launch_zcode();
    match restore {
        Ok(()) => Err(format!("切换失败（{step}），已回滚到切换前的登录状态")),
        Err(e) => Err(format!(
            "切换失败（{step}），且回滚失败（{e}）：请手动将 ~/.zbar/accounts/.last/ 下的备份文件复制到 ZCode 数据目录（ZCode 设置中可见，默认 ~/.zcode/v2/），或重启 ZCode 重新登录"
        )),
    }
}

fn restore_backup(live: &LiveFiles) -> Result<(), String> {
    let backup = backup_dir()?;
    match &live.credentials {
        Some(_) => {
            let raw = fs::read_to_string(backup.join("credentials.json"))
                .map_err(|e| format!("读取备份 credentials.json 失败: {e}"))?;
            atomic_write(&credentials_path()?, &raw)?;
        }
        None => {
            let _ = fs::remove_file(credentials_path()?);
        }
    }
    match &live.config {
        Some(_) => {
            let raw = fs::read_to_string(backup.join("config.json"))
                .map_err(|e| format!("读取备份 config.json 失败: {e}"))?;
            atomic_write(&zcode_config_path()?, &raw)?;
        }
        None => {
            let _ = fs::remove_file(zcode_config_path()?);
        }
    }
    Ok(())
}

/// 删除快照（只删本应用的快照文件，不影响 ZCode 当前登录）。
pub fn remove_account(id: &str) -> Result<(), String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    remove_snapshot_at(&config_dir()?, id)
}

/// 重命名快照（只改 display_name，32 字上限由前端约束，后端再截断兜底）。
/// 改名后置 name_locked=true：后续重捕获不再用默认名覆盖手动命名。
pub fn rename_account(id: &str, new_name: &str) -> Result<AccountMeta, String> {
    let _guard = accounts_lock()
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let mut snap = load_snapshot_at(&config_dir()?, id)
        .ok_or_else(|| "未找到该账号快照".to_string())?;
    let name = new_name.trim().chars().take(32).collect::<String>();
    if name.is_empty() {
        return Err("账号名称不能为空".into());
    }
    snap.display_name = name;
    snap.name_locked = true;
    snap.updated_at = now_ms();
    let meta = AccountMeta::from(&snap);
    save_snapshot_at(&config_dir()?, &snap)?;
    Ok(meta)
}

// ============================================================
// 第四节：多账号额度查询（只读快照 + 并行 HTTP）
// ============================================================

/// account_quotas 返回：单个账号快照的订阅额度查询结果。
#[derive(Debug, Serialize)]
pub struct AccountQuotaEntry {
    pub id: String,
    pub display_name: String,
    pub email: Option<String>,
    /// 账号指纹（user_id，与 quota_history 快照的 account 同一标识）。
    /// 前端用它把"本机+远端合并计算的各账号今日增量"关联回本条目。
    pub fingerprint: String,
    /// 是否当前登录账号（按实时指纹与快照指纹匹配回填，与 list_accounts 同口径）
    pub is_current: bool,
    /// 额度查询结果（失败为 None，错误见 error）
    pub quota: Option<crate::quota::QuotaResult>,
    /// 该账号今日增量 (增量百分比, 今日采样数)（查询失败为 None）。
    /// 由本函数写入的带指纹采样累积计算，当前账号另由 fetch_quota
    /// 30s 一轮补采样，数据比非当前账号更新。
    pub today_delta: Option<(u32, u32)>,
    /// 查询失败原因（quota 为 Some 时为 None；错误串不含 token，原样透传）
    pub error: Option<String>,
}

/// 快照的额度查询凭证：优先捕获时记录的 login_provider（切换事务逐 key 覆盖
/// 不清理，live config/快照可能混入其他账号的同前缀 key，固定序 pick 会错取），
/// 该 key 缺失或 apiKey 失效时回退固定序（老快照无 login_provider 也走这里）。
fn snapshot_credential(snap: &AccountSnapshot) -> Option<(String, String, String)> {
    if let Some(key) = &snap.login_provider {
        if let Some((api_key, base_url)) = snap
            .config_providers
            .get(key)
            .and_then(crate::quota::provider_credential)
        {
            return Some((key.clone(), api_key, base_url));
        }
    }
    crate::quota::pick_coding_plan_api_key(&snap.config_providers)
}

/// 查询全部账号快照各自的订阅额度。
///
/// 数据来源是各快照捕获时保存的 Coding Plan provider（含 apiKey/baseURL），
/// 与当前登录无关——切换账号后仍能查到其他账号的额度。
/// 只读快照目录与实时指纹，全程不持 ACCOUNTS_LOCK（与切换/捕获写事务互不阻塞）；
/// 各账号 HTTP 用 std::thread::scope 并行（单账号 15s 超时独立计时，不叠加）。
///
/// 每个查询成功的账号同时写一条带指纹的额度历史采样（quota_history）：
/// 快照的 account 字段使读写按账号隔离，非当前账号由此获得自己的
/// "今日增量"数据源（5 分钟一轮），且不会污染当前账号的任何读路径。
pub fn account_quotas() -> Result<Vec<AccountQuotaEntry>, String> {
    let base = config_dir()?;
    let snapshots = load_snapshots_at(&base);
    let current_fp = current_fingerprint().map(|fp| fp.user_id);

    // 先在主线程提取元数据与凭证（纯内存解析），线程内只做 HTTP 查询与采样落盘
    let metas: Vec<_> = snapshots
        .iter()
        .map(|snap| {
            (
                snap.id.clone(),
                snap.display_name.clone(),
                snap.email.clone(),
                snap.fingerprint.clone(),
            )
        })
        .collect();
    // 指纹与凭证打包进线程：查询成功后写采样需要知道归属账号
    let jobs: Vec<_> = snapshots
        .iter()
        .map(|snap| (snap.fingerprint.clone(), snapshot_credential(snap)))
        .collect();

    let results: Vec<_> = std::thread::scope(|scope| {
        let handles: Vec<_> = jobs
            .into_iter()
            .map(|(fingerprint, cred)| {
                scope.spawn(move || match cred {
                    // 错误文案刻意不以「未找到 ZCode Coding Plan 凭证」开头：
                    // 该前缀被前端识别为"当前账号未登录"的引导分支，而这里是
                    // "该快照缺凭证"的账号级提示，语义不同，绝不能混用
                    None => (
                        None,
                        Some(
                            "该账号快照无可用 Coding Plan 凭证，请切换到该账号后重新捕获"
                                .to_string(),
                        ),
                    ),
                    Some((_provider_key, token, base_url)) => {
                        match crate::quota::query_quota_with(&token, &base_url) {
                            Ok(q) => {
                                // 写带指纹采样（静默失败，不影响额度查询本身）；
                                // 与 fetch_quota 的 30s 采样经 (ts, account) 防抖去重
                                let snap = crate::quota::snapshot_of(&q, Some(&fingerprint));
                                crate::quota_history::append_snapshot(&snap);
                                (Some(q), None)
                            }
                            // 失败原样透传（query_quota_with 的错误串不含 token）
                            Err(e) => (None, Some(e)),
                        }
                    }
                })
            })
            .collect();
        handles
            .into_iter()
            // 查询线程内无 unwrap/panic 路径，join 失败仅为兜底
            .map(|h| {
                h.join()
                    .unwrap_or((None, Some("额度查询线程异常退出".to_string())))
            })
            .collect()
    });

    // 全部采样落盘后统一计算各账号今日增量（一次历史文件读取）
    let accounts: Vec<String> = metas.iter().map(|m| m.3.clone()).collect();
    let deltas = crate::quota_history::today_deltas(&accounts).unwrap_or_default();

    Ok(metas
        .into_iter()
        .zip(results)
        .map(
            |((id, display_name, email, fingerprint), (quota, error))| AccountQuotaEntry {
                id,
                display_name,
                email,
                fingerprint: fingerprint.clone(),
                is_current: Some(fingerprint.as_str()) == current_fp.as_deref(),
                today_delta: if quota.is_some() {
                    deltas.get(&fingerprint).copied()
                } else {
                    None
                },
                quota,
                error,
            },
        )
        .collect())
}

// ============================================================
// 第五节：ZCode 桌面应用进程控制（macOS / Windows）
// ============================================================

/// 桌面应用可执行名/进程名（macOS）。
#[cfg(target_os = "macos")]
const ZCODE_APP_NAME: &str = "ZCode";

/// ZCode 桌面应用是否在运行（pgrep -x 精确匹配进程名）。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn zcode_running() -> bool {
    std::process::Command::new("pgrep")
        .args(["-x", ZCODE_APP_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

/// 退出 ZCode：osascript 优雅退出 → 轮询 5s → pkill 兜底 → 再轮询 3s。
/// osascript 自身失败（如未授予 Automation 权限）静默降级 pkill。
/// 未运行直接返回 Ok。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn quit_zcode() -> Result<(), String> {
    if !zcode_running() {
        return Ok(());
    }
    let _ = std::process::Command::new("osascript")
        .args(["-e", &format!("quit app \"{}\"", ZCODE_APP_NAME)])
        .output();
    for _ in 0..20 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = std::process::Command::new("pkill")
        .args(["-x", ZCODE_APP_NAME])
        .output();
    for _ in 0..12 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("无法退出 ZCode 桌面应用（可尝试手动退出后重试）".into())
}

/// 启动 ZCode 桌面应用（open -a）。返回是否成功。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(target_os = "macos")]
pub(crate) fn launch_zcode() -> bool {
    std::process::Command::new("open")
        .args(["-a", ZCODE_APP_NAME])
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

// ---------- Windows 实现 ----------
// 思路与 macOS 相同：优雅退出 → 轮询 → 强杀兜底 → 轮询。
// 本应用是 GUI 进程，直接 Command::new 拉起 tasklist/taskkill 会闪控制台
// 黑窗，统一走 CREATE_NO_WINDOW 的 run_hidden。

/// Windows 下的进程/镜像名（tasklist/taskkill 用，不带 .exe）。
#[cfg(windows)]
const ZCODE_EXE_NAME: &str = "ZCode";

/// exe 路径缓存文件（~/.zbar/.zcode-exe-path）。ZCode 可装在任意盘符
/// （如 D:\app\ZCode），切换时趁进程存活捕获一次，之后 ZCode 未运行的
/// 切换也能自动重启。
#[cfg(windows)]
fn zcode_exe_cache_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join(".zcode-exe-path"))
}

/// 静默执行外部命令（CREATE_NO_WINDOW，GUI 进程下不闪控制台黑窗）。
/// pub(crate)：lib.rs 的 show_notification（Windows toast 走 PowerShell）复用。
#[cfg(windows)]
pub(crate) fn run_hidden(program: &str, args: &[&str]) -> Option<std::process::Output> {
    use std::os::windows::process::CommandExt;
    const CREATE_NO_WINDOW: u32 = 0x0800_0000;
    std::process::Command::new(program)
        .args(args)
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .ok()
}

/// ZCode 桌面应用是否在运行（tasklist 按镜像名过滤；CSV + /NH 避开
/// 本地化表头，未命中时输出 "INFO: ..." 行也不含镜像名，不会误判）。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn zcode_running() -> bool {
    let filter = format!("IMAGENAME eq {ZCODE_EXE_NAME}.exe");
    match run_hidden("tasklist", &["/FI", &filter, "/FO", "CSV", "/NH"]) {
        Some(out) if out.status.success() => String::from_utf8_lossy(&out.stdout)
            .to_lowercase()
            .contains(&format!("{}.exe", ZCODE_EXE_NAME.to_lowercase())),
        _ => false,
    }
}

/// 捕获运行中 ZCode 进程的 exe 完整路径（PowerShell Get-Process；
/// 强制 UTF-8 输出——管道重定向下 5.1 默认按 OEM 代码页编码，中文安装
/// 路径会变乱码；失败返回 None，不阻塞退出流程）。
#[cfg(windows)]
fn capture_zcode_exe_path() -> Option<PathBuf> {
    let script = format!(
        "[Console]::OutputEncoding=[System.Text.Encoding]::UTF8; \
          (Get-Process -Name {ZCODE_EXE_NAME} -ErrorAction SilentlyContinue \
          | Where-Object {{ $_.Path }} | Select-Object -First 1 -ExpandProperty Path)"
    );
    let out = run_hidden("powershell", &["-NoProfile", "-Command", &script])?;
    let path = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if path.is_empty() {
        None
    } else {
        Some(PathBuf::from(path))
    }
}

/// 退出 ZCode：先捕获并缓存 exe 路径（供重启用）→ taskkill 发送 WM_CLOSE
/// 优雅退出 → 轮询 5s → taskkill /F 强杀 → 再轮询 3s。未运行直接返回 Ok。
/// 只匹配 ZCode.exe 镜像，CLI 会话进程（node.exe）不受影响，与 macOS
/// pkill -x 的边界一致。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn quit_zcode() -> Result<(), String> {
    if !zcode_running() {
        return Ok(());
    }
    // 趁进程还活着记下安装位置；捕获失败不阻塞（重启退回常见路径探测）
    if let Some(exe) = capture_zcode_exe_path().filter(|p| p.is_file()) {
        if let Ok(cache) = zcode_exe_cache_path() {
            if let Some(parent) = cache.parent() {
                let _ = fs::create_dir_all(parent);
            }
            let _ = fs::write(&cache, exe.to_string_lossy().as_bytes());
        }
    }
    let image = format!("{ZCODE_EXE_NAME}.exe");
    let _ = run_hidden("taskkill", &["/IM", image.as_str()]);
    for _ in 0..20 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    let _ = run_hidden("taskkill", &["/F", "/IM", image.as_str()]);
    for _ in 0..12 {
        if !zcode_running() {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(250));
    }
    Err("无法退出 ZCode 桌面应用（可尝试手动退出后重试）".into())
}

/// 启动 ZCode 桌面应用：优先退出时缓存的 exe 路径，其次常见安装位置；
/// open::that_detached 分离启动，不随本面板退出而终止。
/// pub(crate)：agent_theme（动态壁纸注入）复用。
#[cfg(windows)]
pub(crate) fn launch_zcode() -> bool {
    let exe_suffix = format!("{}.exe", ZCODE_EXE_NAME).to_lowercase();
    let mut candidates: Vec<PathBuf> = vec![];
    if let Ok(cache) = zcode_exe_cache_path() {
        if let Ok(s) = fs::read_to_string(&cache) {
            // 缓存只信任指向 ZCode.exe 的内容，防篡改后借本面板拉起任意程序
            let s = s.trim();
            if !s.is_empty() && s.to_lowercase().ends_with(&exe_suffix) {
                candidates.push(PathBuf::from(s));
            }
        }
    }
    if let Some(base) = std::env::var_os("LOCALAPPDATA") {
        candidates.push(
            PathBuf::from(base)
                .join("Programs")
                .join(ZCODE_EXE_NAME)
                .join(format!("{ZCODE_EXE_NAME}.exe")),
        );
    }
    if let Some(base) = std::env::var_os("ProgramFiles") {
        candidates.push(
            PathBuf::from(base)
                .join(ZCODE_EXE_NAME)
                .join(format!("{ZCODE_EXE_NAME}.exe")),
        );
    }
    for exe in candidates {
        if exe.is_file() && open::that_detached(&exe).is_ok() {
            return true;
        }
    }
    false
}

// Linux：桌面端进程控制暂未实现，切换事务在 quit 环节直接报错引导手动操作。
#[cfg(target_os = "linux")]
fn quit_zcode() -> Result<(), String> {
    Err("当前平台暂不支持自动切换：请手动退出 ZCode 后重试".into())
}

#[cfg(target_os = "linux")]
fn launch_zcode() -> bool {
    false
}

// ============================================================
// 存储层单测（temp 目录，不触碰真实 ~/.zbar 与 ~/.zcode）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 每个测试独立的临时基目录，结束时清理。
    struct TempBase(PathBuf);

    impl TempBase {
        fn new(tag: &str) -> Self {
            let dir = std::env::temp_dir().join(format!(
                "zbar-accounts-test-{tag}-{}-{}",
                std::process::id(),
                now_ms()
            ));
            fs::create_dir_all(&dir).expect("创建临时目录失败");
            TempBase(dir)
        }
    }

    impl Drop for TempBase {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn sample_snapshot(id: &str, name: &str) -> AccountSnapshot {
        AccountSnapshot {
            version: 1,
            id: id.to_string(),
            fingerprint: id.to_string(),
            display_name: name.to_string(),
            email: Some("a@b.c".into()),
            created_at: 1000,
            updated_at: 1000,
            credentials_raw: "{\"k\":\"v\"}".into(),
            config_providers: Map::new(),
            login_provider: None,
            name_locked: false,
        }
    }

    #[test]
    fn save_list_rename_remove_roundtrip() {
        let base = TempBase::new("roundtrip");
        save_snapshot_at(&base.0, &sample_snapshot("user1", "一号")).unwrap();
        let mut later = sample_snapshot("user2", "二号");
        later.created_at = 2000; // 保证排序确定（不同 created_at）
        save_snapshot_at(&base.0, &later).unwrap();

        let list = load_meta_list_at(&base.0);
        assert_eq!(list.len(), 2, "两个快照都应被列出");
        assert_eq!(list[0].id, "user1", "按 created_at 升序");

        // 加载单个
        let snap = load_snapshot_at(&base.0, "user1").expect("应能读回快照");
        assert_eq!(snap.display_name, "一号");
        assert_eq!(snap.credentials_raw, "{\"k\":\"v\"}");

        // 非法 id 直接 None（防路径穿越）
        assert!(load_snapshot_at(&base.0, "../etc").is_none());
        assert!(load_snapshot_at(&base.0, "a/b").is_none());

        // 重命名（复用生产 rename 的底层：load→改→save）
        let base_path = base.0.clone();
        let _guard = accounts_lock()
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let mut snap = load_snapshot_at(&base_path, "user1").unwrap();
        snap.display_name = "新名字".into();
        save_snapshot_at(&base_path, &snap).unwrap();
        assert_eq!(load_meta_list_at(&base_path)[0].display_name, "新名字");

        // 删除
        remove_snapshot_at(&base_path, "user1").unwrap();
        assert_eq!(load_meta_list_at(&base_path).len(), 1);
        // 再删报错
        assert!(remove_snapshot_at(&base_path, "user1").is_err());
    }

    #[test]
    fn corrupted_snapshot_is_skipped() {
        let base = TempBase::new("corrupt");
        save_snapshot_at(&base.0, &sample_snapshot("good", "好的")).unwrap();
        // 写一个损坏的快照文件
        let dir = base.0.join("accounts");
        fs::write(dir.join("broken.account.json"), "not a json").unwrap();
        // 非快照后缀的文件不参与扫描
        fs::write(dir.join("notes.txt"), "hello").unwrap();

        let list = load_meta_list_at(&base.0);
        assert_eq!(list.len(), 1, "损坏快照被跳过，正常快照保留");
        assert_eq!(list[0].id, "good");
    }

    #[test]
    fn permissions_are_hardened() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let base = TempBase::new("perm");
            save_snapshot_at(&base.0, &sample_snapshot("u1", "n")).unwrap();

            let dir = base.0.join("accounts");
            let mode = fs::metadata(&dir).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o700, "快照目录应为 0700");

            let file = dir.join("u1.account.json");
            let mode = fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600, "快照文件应为 0600");
        }
    }

    #[test]
    fn snapshot_id_filters_characters() {
        let fp = Fingerprint {
            user_id: "abc/DEF-123".into(),
            email: None,
            display_name: None,
        };
        assert_eq!(snapshot_id_of(Some(&fp)), "abcDEF-123");
        // 全非法字符退化为 unknown-
        let fp2 = Fingerprint {
            user_id: "/./".into(),
            email: None,
            display_name: None,
        };
        assert!(snapshot_id_of(Some(&fp2)).starts_with("unknown-"));
        assert!(snapshot_id_of(None).starts_with("unknown-"));
    }

    /// 快照默认名优先级：昵称 > 邮箱 > 账号-{id前8}
    #[test]
    fn default_display_name_priority() {
        let with_nick = Fingerprint {
            user_id: "u1".into(),
            email: Some("a@b.c".into()),
            display_name: Some("小智".into()),
        };
        assert_eq!(
            default_display_name(Some(&with_nick), Some("a@b.c"), "u1"),
            "小智"
        );
        let no_nick = Fingerprint {
            user_id: "u1".into(),
            email: Some("a@b.c".into()),
            display_name: None,
        };
        assert_eq!(default_display_name(Some(&no_nick), Some("a@b.c"), "u1"), "a@b.c");
        assert_eq!(default_display_name(None, None, "abcdefgh1234"), "账号-abcdefgh");
    }

    /// name_locked 行为：重捕获仅当旧快照未锁定时才刷新 display_name
    #[test]
    fn recapture_respects_name_locked() {
        // 未锁定：重捕获用新默认名覆盖（复用生产 capture 的 upsert 分支逻辑）
        let base = TempBase::new("relock-false");
        let mut old = sample_snapshot("user1", "旧名字");
        old.name_locked = false;
        save_snapshot_at(&base.0, &old).unwrap();

        let fp = Fingerprint {
            user_id: "user1".into(),
            email: Some("new@b.c".into()),
            display_name: Some("新昵称".into()),
        };
        let mut refreshed = old.clone();
        refreshed.display_name = default_display_name(Some(&fp), refreshed.email.as_deref(), "user1");
        refreshed.email = fp.email.clone();
        refreshed.updated_at = 2000;
        save_snapshot_at(&base.0, &refreshed).unwrap();
        let got = load_snapshot_at(&base.0, "user1").unwrap();
        assert_eq!(got.display_name, "新昵称", "未锁定时重捕获刷新默认名");
        assert_eq!(got.email.as_deref(), Some("new@b.c"), "email 照旧刷新");
        assert_eq!(got.created_at, 1000, "created_at 保留");

        // 已锁定：重捕获保留手动命名（email/fingerprint 等其余字段照旧刷新）
        let base2 = TempBase::new("relock-true");
        let mut locked = sample_snapshot("user2", "手动名");
        locked.name_locked = true;
        save_snapshot_at(&base2.0, &locked).unwrap();

        let fp2 = Fingerprint {
            user_id: "user2".into(),
            email: Some("new@b.c".into()),
            display_name: Some("新昵称".into()),
        };
        let mut refreshed2 = locked.clone();
        // 生产 capture 的分支：name_locked=true 直接沿用旧 display_name
        refreshed2.display_name = locked.display_name.clone();
        refreshed2.email = fp2.email.clone();
        refreshed2.fingerprint = fp2.user_id.clone();
        refreshed2.updated_at = 2000;
        save_snapshot_at(&base2.0, &refreshed2).unwrap();
        let got2 = load_snapshot_at(&base2.0, "user2").unwrap();
        assert_eq!(got2.display_name, "手动名", "锁定名不被重捕获覆盖");
        assert_eq!(got2.email.as_deref(), Some("new@b.c"), "锁定时 email 仍刷新");
        assert_eq!(got2.fingerprint, "user2");
    }

    /// 老快照（无 name_locked 字段）反序列化为 false，保持原兼容行为
    #[test]
    fn legacy_snapshot_without_name_lock_defaults_false() {
        let base = TempBase::new("legacy");
        let dir = base.0.join("accounts");
        fs::create_dir_all(&dir).unwrap();
        // 手写无 name_locked 字段的快照（模拟升级前的老文件）
        fs::write(
            dir.join("old.account.json"),
            r#"{
                "version": 1,
                "id": "old",
                "fingerprint": "old",
                "display_name": "老账号",
                "email": null,
                "created_at": 1000,
                "updated_at": 1000,
                "credentials_raw": "{}",
                "config_providers": {}
            }"#,
        )
        .unwrap();
        let snaps = load_snapshots_at(&base.0);
        assert_eq!(snaps.len(), 1);
        assert_eq!(snaps[0].name_locked, false, "老快照缺字段默认未锁定");
        assert_eq!(snaps[0].display_name, "老账号");
    }

    /// load_snapshots_at 返回完整快照（含 credentials_raw / config_providers）并按 created_at 升序
    #[test]
    fn load_snapshots_returns_full_content_sorted() {
        let base = TempBase::new("full-snaps");
        save_snapshot_at(&base.0, &sample_snapshot("user1", "一号")).unwrap();
        let mut later = sample_snapshot("user2", "二号");
        later.created_at = 2000;
        later.credentials_raw = "{\"k\":\"v2\"}".into();
        save_snapshot_at(&base.0, &later).unwrap();

        let snaps = load_snapshots_at(&base.0);
        assert_eq!(snaps.len(), 2);
        assert_eq!(snaps[0].id, "user1", "按 created_at 升序");
        assert_eq!(snaps[0].credentials_raw, "{\"k\":\"v\"}");
        assert_eq!(snaps[1].credentials_raw, "{\"k\":\"v2\"}");
    }

    #[test]
    fn coding_plan_providers_filters_keys() {
        let config = r#"{
            "provider": {
                "builtin:bigmodel": {"apiKey": ""},
                "builtin:bigmodel-coding-plan": {"apiKey": "k1"},
                "builtin:zai-coding-plan": {"apiKey": ""},
                "uuid-custom": {"apiKey": "k2"}
            }
        }"#;
        let providers = coding_plan_providers(Some(config));
        assert_eq!(providers.len(), 2, "只保留 key 含 coding-plan 的 provider");
        assert!(providers.contains_key("builtin:bigmodel-coding-plan"));
        assert!(providers.contains_key("builtin:zai-coding-plan"));
        // apiKey 为空的 coding-plan 也保留
        assert_eq!(
            providers.get("builtin:zai-coding-plan").unwrap().get("apiKey"),
            Some(&serde_json::json!(""))
        );
        // 缺失/坏格式 → 空 map
        assert!(coding_plan_providers(None).is_empty());
        assert!(coding_plan_providers(Some("bad json")).is_empty());
    }

    /// 快照凭证选择：login_provider 优先——即使混入的其他账号 key 在固定序里
    /// 更靠前，也必须用捕获时记录的那个；字段缺失或其 apiKey 失效则回退固定序
    #[test]
    fn snapshot_credential_prefers_login_provider() {
        let mut snap = sample_snapshot("u1", "一号");
        snap.config_providers = serde_json::from_str(
            r#"{
                "builtin:bigmodel-coding-plan": {
                    "options": {"apiKey": "keyA-other-account", "baseURL": ""}
                },
                "builtin:zai-coding-plan": {
                    "options": {"apiKey": "keyB-this-account", "baseURL": "https://api.z.ai/api/anthropic"}
                }
            }"#,
        )
        .unwrap();

        // 无 login_provider（老快照）→ 固定序选 bigmodel
        let got = snapshot_credential(&snap).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
        assert_eq!(got.1, "keyA-other-account");

        // 记录了 zai → 必须用 zai 的凭证（bigmodel 虽在固定序优先也不得命中）
        snap.login_provider = Some("builtin:zai-coding-plan".into());
        let got = snapshot_credential(&snap).unwrap();
        assert_eq!(got.0, "builtin:zai-coding-plan");
        assert_eq!(got.1, "keyB-this-account");
        assert_eq!(got.2, "https://api.z.ai/api/anthropic");

        // login_provider 对应 key 的 apiKey 变为空 → 回退固定序
        snap.config_providers
            .get_mut("builtin:zai-coding-plan")
            .unwrap()["options"]["apiKey"] = serde_json::json!("");
        let got = snapshot_credential(&snap).unwrap();
        assert_eq!(got.0, "builtin:bigmodel-coding-plan");
    }
}


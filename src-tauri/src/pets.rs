//! 自定义宠物（宠物功能第三阶段）：导入并管理符合 Petdex 格式的用户自制宠物。
//!
//! ## Petdex 规范（来源：GitHub crafter-station/petdex，实现前逐文件核实）
//! 以下事实全部取自仓库源码，非猜测：
//! - **分发形态**：一只宠物 = 一个目录，内含两个文件——`pet.json` + `spritesheet.webp`
//!   （或 `.png`）。zip 只是画廊下载/提交的传输封装（包内同样为根目录两文件，
//!   见 packages/petdex-cli/bin/petdex.ts 的 readZipCandidate）。CLI 安装落盘为
//!   `~/.petdex/pets/<slug>/` 与 `~/.codex/pets/<slug>/`。
//! - **pet.json 字段**（CLI 实际读取的 4 个字段，见 bin/petdex.ts 的
//!   readFolderCandidate / submitOne）：`id`（string，唯一标识，slug 化后作目录名）、
//!   `displayName`（string，显示名）、`description`（string）、`spriteVersionNumber`
//!   （1 | 2，可省略 = 1）。其余字段（tags/vibes/kind 等）仅入库展示，渲染不消费。
//!   Windows 桌面端额外兼容 `name` 字段（displayName || name）。
//! - **精灵图集**：每格 192×208。v1 = 8 列 × 9 行 = 1536×1872；v2 = 8 列 × 11 行 =
//!   1536×2288（"ChatGPT pet exports are already the v2 shape"，见 CLI README）。
//!   官方接受两版或其整数倍缩放（"a clean scale of either"）。
//! - **逐行状态定义**（src/lib/pet-states.ts，web/Zig 桌面/Windows 桌面三端一致）：
//!   | 行 | 状态 | 帧数 | 循环时长 |
//!   |----|------|------|----------|
//!   | 0 | idle | 6 | 1100ms |
//!   | 1 | running-right | 8 | 1060ms |
//!   | 2 | running-left | 8 | 1060ms |
//!   | 3 | waving | 4 | 700ms |
//!   | 4 | jumping | 5 | 840ms |
//!   | 5 | failed | 8 | 1220ms |
//!   | 6 | waiting | 6 | 1010ms |
//!   | 7 | running | 6 | 820ms |
//!   | 8 | review | 6 | 1030ms |
//!   v2 的第 9、10 行（下标 9/10）在仓库全部渲染器中均无消费者，导入时忽略。
//! - **帧率信息**：pet.json 不携带帧率/每状态帧数——每状态帧数与循环时长由
//!   网格布局与渲染器共享常量隐含（"The desktop app currently hardcodes these
//!   values rather than reading them from pet.json"）。
//!
//! ## ZBar 内部格式（导入时从 Petdex 转换）
//! 导入目录 `~/.zbar/pets/<id>/`，内含 `pet.json`（内部格式）+ `sheet.<webp|png>`
//! （图集字节原样保留）。内部 pet.json 形如：
//! ```json
//! { "id": "boba", "name": "Boba", "format": "petdex-v1",
//!   "cols": 8, "rows": 9, "frameW": 192, "frameH": 208, "image": "sheet.webp",
//!   "states": { "sleeping": { "row": 6, "frames": 6, "frameMs": 800 }, ... } }
//! ```
//! 七状态映射（V6：五状态 + tool_running/failed）与 frameMs 默认值在导入时
//! 写死（见 build_default_states），states 为显式配置，用户可手改 pet.json
//! 微调（加载时做边界收敛）。旧五状态 pet.json（V6 前导入）读取时缺
//! tool_running/failed 键由 normalize_meta 用默认行补齐（平滑升级，无需
//! 重新导入）。V9 细分键 thinking/walking（动作语义细分）例外：有则直读
//! 保留、缺则不补——细分映射只随内置智谱娘 pet.json 分发（经内置元数据
//! 升级机制落到用户库），缺键形象由 pet-core.js 的 CUSTOM_STATE_FALLBACK
//! 回退 working 帧，通用/老宠物行为与 V8 完全一致。
//!
//! ## 两条消费链路
//! - **注入版**（皮肤已安装时）：PetConfig.style = `custom:<id>` 则由
//!   [`sync_theme_custom_pet`] 把该宠物物化为主题目录的 `pet-custom.js`
//!   （`window.__ZBAR_PET_CUSTOM__ = { v:1, meta, dataUri }`），注入版壳读到
//!   custom:* 形象时按需加载一次并作为 customAsset 传入 pet-core。
//! - **悬浮窗**：`get_custom_pet_asset` 命令按 id 读回 meta + dataUri，
//!   pet-main.ts 在 style 为 custom:* 时 invoke 获取后传给 pet-core。
//!   （宠物配置已统一收敛到 pet.json/PetConfig，两形态共用同一份选中形象。）
//!
//! ## 内置形象（V8 起默认）
//! 「智谱 Z 娘」（id = `zhipu-z-niang`）随安装包分发（src-tauri/assets/
//! pets/ 下内嵌 pet.json + sheet.webp，编译期 include_bytes!），启动时由
//! [`ensure_builtin_pet`] 释放到宠物库：图集缺失/损坏才落盘（约 2.5MB
//! 且不随版本变化，已存在不覆盖）；pet.json 的状态映射属软件管理范畴
//! （V9 细分 thinking/walking 键等随版本演进），库内字节与内置不一致时
//! 覆盖升级（.tmp + rename 原子写，见 [`ensure_builtin_pet_in`]）；默认
//! 选中（pet.rs 的 DEFAULT_PET_STYLE = `custom:zhipu-z-niang`）且不可
//! 删除（delete_custom_pet 对内置 id 拒绝）。cat/bot 两个旧内建字符网格
//! 形象已随 V8 渲染收敛移除（pet-core.js 改 customAsset-only）。

use base64::Engine;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read};
use std::path::{Path, PathBuf};

/// 自定义宠物库根目录：~/.zbar/pets
pub const PETS_DIR: &str = "pets";
/// 每只宠物目录内的元信息文件名（内部格式）
pub const PET_META_FILE: &str = "pet.json";
/// 图集文件前缀（实际为 sheet.webp / sheet.png，扩展名随导入源保留）
pub const SHEET_PREFIX: &str = "sheet";
/// 注入版物化文件名（落在 ~/.zbar/agent-themes/zcode/ 下，与 pet.js 同目录）
pub const PET_CUSTOM_JS: &str = "pet-custom.js";
/// pet_style / PetConfig.style 中的自定义形象前缀
pub const CUSTOM_STYLE_PREFIX: &str = "custom:";
/// 缩略图适配框（Petdex 帧 192×208 ≈ 64×69，取 64×70 容器内等比适配）
pub const THUMB_W: u32 = 64;
pub const THUMB_H: u32 = 70;
/// 导入图集体积上限（与 Petdex 桌面端 MAX_PET_BYTES 同口径，防超大图拖垮渲染）
pub const MAX_SHEET_BYTES: u64 = 16 * 1024 * 1024;
/// 单次读取上限（zip 条目与旁路图集共用）：导入源可能被恶意构造
/// （zip bomb 解压膨胀），必须先封顶再读，避免全量膨胀进内存后才被
/// 体积校验拒绝
pub const MAX_READ_BYTES: u64 = 64 * 1024 * 1024;

/// 内置宠物 id（「智谱 Z 娘」）：随安装包分发、默认选中、不可删除。
/// 对应的内置 style 值为 `custom:zhipu-z-niang`（见 pet.rs 的
/// DEFAULT_PET_STYLE，两处经 concat! 同源拼接）
pub const BUILTIN_PET_ID: &str = "zhipu-z-niang";
/// 内置宠物元信息（编译期从 src-tauri/assets/pets/ 内嵌）
const BUILTIN_PET_JSON: &[u8] = include_bytes!("../assets/pets/zhipu-z-niang/pet.json");
/// 内置宠物精灵图集（同上，约 2.5MB webp）
const BUILTIN_PET_SHEET: &[u8] = include_bytes!("../assets/pets/zhipu-z-niang/sheet.webp");

// ============================================================
// 内部格式数据结构
// ============================================================

/// 单状态的行配置：图集第 row 行、该状态循环 frames 帧、每帧停留 frameMs
/// （typing 允许数组 = 按速度三档，与内建形象同语义）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CustomPetStateDef {
    pub row: u32,
    pub frames: u32,
    /// number 或 [number, number, number]（typing 三档）
    #[serde(rename = "frameMs")]
    pub frame_ms: serde_json::Value,
}

/// 内部格式的宠物元信息（落盘 ~/.zbar/pets/<id>/pet.json，serde camelCase）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CustomPetMeta {
    pub id: String,
    pub name: String,
    /// 来源格式："petdex-v1"（8×9）| "petdex-v2"（8×11）
    pub format: String,
    pub cols: u32,
    pub rows: u32,
    pub frame_w: u32,
    pub frame_h: u32,
    /// 图集文件名（sheet.webp / sheet.png）
    pub image: String,
    /// 状态行配置（键：sleeping/idle/working/typing/celebrating/
    /// tool_running/failed 七键恒在（V6 起含后两个，缺键补默认行）；
    /// V9 细分键 thinking/walking 有则直读保留、缺不补——通用 v2 宠物
    /// 作者自命名的行语义各不相同，细分映射只随内置 pet.json 分发，
    /// 缺键形象由渲染端 CUSTOM_STATE_FALLBACK 回退 working 帧）
    pub states: BTreeMap<String, CustomPetStateDef>,
}

/// Petdex 行 → ZBar 七状态的默认映射（导入时写死进 states）。
/// 行号/帧数来自 petdex src/lib/pet-states.ts 的逐行定义；frameMs 默认值
/// 沿用内建形象节奏，理由见函数头注释。
///
/// | ZBar 状态 | Petdex 行 | 帧 | 说明 |
/// |-----------|-----------|----|------|
/// | sleeping | 6 waiting | 6 | Petdex 无睡眠行，取"耐心等待的闲置变体"语义最接近 |
/// | idle | 0 idle | 6 | 中性呼吸/眨眼循环 |
/// | working | 8 review | 6 | "专注检查/思考循环" ≈ 思考状态 |
/// | typing | 7 running | 6 | "原地快跑循环" ≈ 高速输出观感 |
/// | celebrating | 4 jumping | 5 | "起跳庆祝"（预备/起跳/顶点/下落/落定） |
/// | tool_running | 1 running-right | 8 | V6："向右奔跑" ≈ 替主人跑腿执行工具 |
/// | failed | 5 failed | 8 | V6：官方失败动画（垂头沮丧） |
/// 未映射行：2 running-left、3 waving（及 v2 的 9/10 行）。
pub(crate) fn build_default_states(cols: u32) -> BTreeMap<String, CustomPetStateDef> {
    let num = |v: u32| serde_json::Value::from(v);
    let def = |row: u32, frames: u32, ms: serde_json::Value| CustomPetStateDef {
        row,
        frames: frames.min(cols.max(1)),
        frame_ms: ms,
    };
    BTreeMap::from([
        // frameMs 默认沿用内建形象（cat/bot 同值）：Petdex 各行授权循环时长
        // 700~1220ms（每帧约 120~175ms）适合其短促状态，但 ZBar 的语义不同
        // ——sleeping 长时驻留须慢、typing 按 token 增速分三档，照搬 Petdex
        // 时值会让沉睡宠物狂闪，故节奏取内建口径，用户可手改 pet.json 调整。
        (
            "sleeping".to_string(),
            def(6, 6, num(800)), /* 行 6 waiting */
        ),
        ("idle".to_string(), def(0, 6, num(450))),
        ("working".to_string(), def(8, 6, num(300))), /* 行 8 review */
        (
            "typing".to_string(),
            def(
                7, /* 行 7 running */
                6,
                serde_json::json!([220, 150, 95]),
            ),
        ),
        ("celebrating".to_string(), def(4, 5, num(160))), /* 行 4 jumping */
        // V6 新状态：tool_running 走行 1 running-right（8 帧跑动循环，
        // "跑腿干活"语义；节奏取 typing 中档 150ms——Petdex 官方循环
        // 1060ms/8 帧 ≈ 132.5ms/帧，同量级的跑动观感）；failed 走行 5
        // failed（8 帧官方失败动画；节奏取 working 的 300ms 中速——
        // Petdex 官方 1220ms/8 帧 ≈ 152.5ms/帧，沮丧展示不狂闪）
        (
            "tool_running".to_string(),
            def(1, 8, num(150)), /* 行 1 running-right */
        ),
        ("failed".to_string(), def(5, 8, num(300))), /* 行 5 failed */
    ])
}

/// 加载时收敛用户手改过的 states（坏值不抛错，回到安全边界）：
/// row 夹进行数、frames 夹进列数且 ≥1、frameMs 校验为正数或非空正数数组
/// （非法回默认节奏 400 / typing [220,150,95]）。V6 兼容：旧五状态
/// pet.json 缺 tool_running/failed 键时用默认行补齐（None => fallback，
/// 导入老数据平滑升级，无需重新导入）。V9 细分键 thinking/walking：
/// pet.json 有则直读收敛保留（不覆盖已有键——内置智谱娘的细分映射随
/// 版本演进，经 ensure_builtin_pet 的元数据升级落到用户库）、缺则不补
/// 默认行（通用 v2 宠物的行语义由作者自命名，没有可靠的默认细分行，
/// 渲染端 pet-core.js 的 CUSTOM_STATE_FALLBACK 回退 working 帧，老宠物
/// 与用户自定义宠物行为与 V8 完全一致）。
pub(crate) fn normalize_meta(mut meta: CustomPetMeta) -> CustomPetMeta {
    if meta.cols == 0 {
        meta.cols = 8;
    }
    if meta.rows == 0 {
        meta.rows = 9;
    }
    if meta.frame_w == 0 || meta.frame_h == 0 {
        meta.frame_w = 192;
        meta.frame_h = 208;
    }
    if meta.image.trim().is_empty() {
        meta.image = format!("{SHEET_PREFIX}.webp");
    }
    let defaults = build_default_states(meta.cols);
    let mut states = BTreeMap::new();
    let fix = |d: &CustomPetStateDef,
               default_ms: serde_json::Value|
     -> CustomPetStateDef {
        CustomPetStateDef {
            row: d.row.min(meta.rows.saturating_sub(1)),
            frames: d.frames.clamp(1, meta.cols),
            // 非法 frameMs 回该状态的内建默认节奏（sleeping 慢、typing 三档）
            frame_ms: if frame_ms_valid(&d.frame_ms) {
                d.frame_ms.clone()
            } else {
                default_ms
            },
        }
    };
    for key in [
        "sleeping",
        "idle",
        "working",
        "typing",
        "celebrating",
        "tool_running",
        "failed",
    ] {
        let fallback = defaults.get(key).cloned().unwrap();
        let fixed = match meta.states.get(key) {
            Some(d) => fix(d, fallback.frame_ms),
            None => fallback,
        };
        states.insert(key.to_string(), fixed);
    }
    // V9 细分键：有则直读收敛（不覆盖已有键），缺则不补（渲染端回退
    // working；无默认行故非法 frameMs 回 400——pet-core customStateDef
    // 的兜底节奏同值）
    for key in ["thinking", "walking"] {
        if let Some(d) = meta.states.get(key) {
            let fixed = fix(d, serde_json::Value::from(400));
            states.insert(key.to_string(), fixed);
        }
    }
    meta.states = states;
    meta
}

/// frameMs 形态校验：正数（>0 且有限）或非空正数数组
fn frame_ms_valid(v: &serde_json::Value) -> bool {
    let ok_num = v.as_f64().is_some_and(|n| n > 0.0 && n.is_finite());
    let ok_arr = v.as_array().is_some_and(|a| {
        !a.is_empty() && a.iter().all(|x| x.as_f64().is_some_and(|n| n > 0.0 && n.is_finite()))
    });
    ok_num || ok_arr
}

// ============================================================
// 目录与路径
// ============================================================

/// 自定义宠物库根目录（~/.zbar/pets）
pub fn pets_root() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join(PETS_DIR))
}

/// id 合法性（防路径遍历）：小写字母/数字/连字符，不以连字符开头，≤64 字符
pub(crate) fn valid_pet_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        && !id.starts_with('-')
}

/// pet_style / PetConfig.style 是否指向自定义形象，是则返回其 id
pub fn custom_style_id(style: &str) -> Option<&str> {
    style
        .strip_prefix(CUSTOM_STYLE_PREFIX)
        .filter(|id| valid_pet_id(id))
}

// ============================================================
// slug 派生（与 petdex CLI 同款：非拉丁 id 回退 fnv 哈希 slug）
// ============================================================

fn slugify(value: &str) -> String {
    value
        .to_lowercase()
        .trim()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect::<String>()
        .chars()
        .take(40)
        .collect::<String>()
        .trim_matches('-')
        .to_string()
}

fn fnv1a_hex(seed: &str) -> String {
    let mut hash: u32 = 0x811c_9dc5;
    for b in seed.as_bytes() {
        hash ^= *b as u32;
        hash = hash.wrapping_mul(0x0100_0193);
    }
    format!("pet-{:07x}", hash)
}

/// 派生导入目录 id：pet.json.id → displayName → 文件名，全空回 fnv 哈希。
/// 【遗留风险标注】非拉丁名（中文/emoji）slug 化后塌缩为哈希 id——同名
/// 宠物内容不同仍会互相覆盖（整体替换语义），显示名保留不受影响；若未来
/// 需要并存可改为追加内容指纹后缀（petdex 官方同为 slug+短哈希形态）。
pub(crate) fn derive_pet_id(candidates: &[&str]) -> String {
    for c in candidates {
        let s = slugify(c);
        if !s.is_empty() {
            return s;
        }
    }
    let seed = candidates.join(" ").trim().to_string();
    fnv1a_hex(if seed.is_empty() { "zbar-pet" } else { &seed })
}

// ============================================================
// 导入：三种入口（zip / pet.json / 裸图集）
// ============================================================

/// 读取封顶（zip 条目与旁路文件共用）：Read::take 限到上限 +1 字节，
/// 超限即报中文错误（P1-1：zip bomb 在膨胀进内存前被拦截）
fn read_capped(r: impl Read, what: &str) -> Result<Vec<u8>, String> {
    let mut b = Vec::new();
    r.take(MAX_READ_BYTES + 1)
        .read_to_end(&mut b)
        .map_err(|e| format!("读取{what}失败：{e}"))?;
    if b.len() as u64 > MAX_READ_BYTES {
        return Err(format!("{what}过大（超过 64MB 上限）"));
    }
    Ok(b)
}

/// zip 包内提取（pet.json 可选 + spritesheet 必需）：
/// 兼容根目录两文件与单层目录包裹两种包型；跳过路径不安全的条目；
/// 条目内容经读取封顶防解压炸弹。
fn extract_zip_parts(bytes: &[u8]) -> Result<(Option<String>, Vec<u8>), String> {
    let mut zip = zip::ZipArchive::new(Cursor::new(bytes))
        .map_err(|e| format!("zip 包无法读取：{e}"))?;
    let mut pet_json: Option<String> = None;
    let mut sheet: Option<Vec<u8>> = None;
    for i in 0..zip.len() {
        let mut f = zip
            .by_index(i)
            .map_err(|e| format!("读取 zip 条目失败：{e}"))?;
        if f.is_dir() {
            continue;
        }
        // 路径安全：仅接受相对封闭路径（拦截 ../ 与绝对路径注入）
        let Some(enclosed) = f.enclosed_name() else {
            continue;
        };
        let base = enclosed.to_string_lossy().replace('\\', "/");
        let Some(name) = base.rsplit('/').next() else {
            continue;
        };
        let lower = name.to_ascii_lowercase();
        if pet_json.is_none() && lower == "pet.json" {
            let raw = read_capped(&mut f, "包内 pet.json")?;
            pet_json = Some(
                String::from_utf8(raw)
                    .map_err(|_| "包内 pet.json 不是有效的 UTF-8 文本".to_string())?,
            );
        } else if sheet.is_none()
            && ["spritesheet.webp", "spritesheet.png", "sprite.webp", "sprite.png"]
                .contains(&lower.as_str())
        {
            sheet = Some(read_capped(&mut f, "包内精灵图集")?);
        }
    }
    let sheet = sheet.ok_or("zip 包内未找到精灵图集（spritesheet.webp / spritesheet.png）")?;
    Ok((pet_json, sheet))
}

/// 解析 Petdex pet.json 文本：读取 id / displayName（兼容 name）/
/// spriteVersionNumber（1|2，缺省 1，其它值报错，与 petdex CLI 同口径）。
fn parse_petdex_json(text: &str) -> Result<(Option<String>, String, Option<u8>), String> {
    let v: serde_json::Value = serde_json::from_str(text).map_err(|e| format!("pet.json 不是有效 JSON：{e}"))?;
    let obj = v.as_object().ok_or("pet.json 内容应为 JSON 对象")?;
    let id = obj.get("id").and_then(|x| x.as_str()).map(str::to_string);
    let name = obj
        .get("displayName")
        .or_else(|| obj.get("name"))
        .and_then(|x| x.as_str())
        .unwrap_or("")
        .trim()
        .chars()
        .take(60)
        .collect::<String>();
    let version = match obj.get("spriteVersionNumber") {
        None | Some(serde_json::Value::Null) => None,
        Some(x) => match x.as_u64() {
            Some(1) => Some(1),
            Some(2) => Some(2),
            _ => return Err("pet.json 的 spriteVersionNumber 仅支持 1 或 2".into()),
        },
    };
    Ok((id, name, version))
}

/// 图集网格判定：宽度按 8 列整除，行数 9（v1）/ 11（v2）按 hint 与实际
/// 尺寸双重裁定（pet.json 版本声明与图集不符时以能整除者为准，与 petdex
/// "clean scale" 容忍口径一致）。返回 (rows, format)。
pub(crate) fn grid_of(width: u32, height: u32, version_hint: Option<u8>) -> Result<(u32, &'static str), String> {
    if width == 0 || height == 0 {
        return Err("图集尺寸无效".into());
    }
    if width % 8 != 0 {
        return Err(format!("图集宽度 {width} 无法按 8 列整除（Petdex 规范为 8 列网格）"));
    }
    let ok9 = height % 9 == 0;
    let ok11 = height % 11 == 0;
    let pick = |rows: u32, fmt: &'static str| Ok((rows, fmt));
    match version_hint {
        Some(2) => {
            if ok11 {
                pick(11, "petdex-v2")
            } else if ok9 {
                // 声明 v2 但图集实为 v1 尺寸：按实际网格裁定
                pick(9, "petdex-v1")
            } else {
                Err(format!("图集高度 {height} 无法按 9 或 11 行整除（v1=8×9，v2=8×11）"))
            }
        }
        _ => {
            if ok9 {
                pick(9, "petdex-v1")
            } else if ok11 {
                pick(11, "petdex-v2")
            } else {
                Err(format!("图集高度 {height} 无法按 9 或 11 行整除（v1=8×9，v2=8×11）"))
            }
        }
    }
}

/// 嗅探图集格式并读取尺寸：返回 (扩展名, 宽, 高)。png/webp 以文件内容为准
/// （扩展名可能缺失或与内容不符）。
fn sniff_sheet(bytes: &[u8]) -> Result<(&'static str, u32, u32), String> {
    let reader = image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| format!("读取图集失败：{e}"))?;
    let format = reader
        .format()
        .ok_or("无法识别图集格式（仅支持 PNG / WebP）")?;
    let ext = match format {
        image::ImageFormat::Png => "png",
        image::ImageFormat::WebP => "webp",
        _ => return Err("仅支持 PNG / WebP 格式的精灵图集".into()),
    };
    let (w, h) = reader
        .into_dimensions()
        .map_err(|e| format!("解析图集尺寸失败：{e}"))?;
    Ok((ext, w, h))
}

/// 导入核心（目录显式版，单元测试复用）：
/// `pet_json_text` 为 None 表示裸图集导入（用默认映射与文件名生成）。
/// 写入流程：临时目录写全 → 移除旧目录 → 原子改名，杜绝半截状态。
pub(crate) fn import_pet_in(
    root: &Path,
    pet_json_text: Option<&str>,
    sheet_bytes: &[u8],
    fallback_name: &str,
) -> Result<CustomPetMeta, String> {
    if sheet_bytes.is_empty() {
        return Err("图集文件为空".into());
    }
    if sheet_bytes.len() as u64 > MAX_SHEET_BYTES {
        return Err("图集文件超过 16MB 上限".into());
    }
    let (petdex_id, petdex_name, version_hint) = match pet_json_text {
        Some(text) => parse_petdex_json(text)?,
        None => (None, String::new(), None),
    };
    let (ext, width, height) = sniff_sheet(sheet_bytes)?;
    let (rows, format) = grid_of(width, height, version_hint)?;
    let frame_w = width / 8;
    let frame_h = height / rows;

    let id = derive_pet_id(&[
        petdex_id.as_deref().unwrap_or(""),
        &petdex_name,
        fallback_name,
    ]);
    if !valid_pet_id(&id) {
        return Err(format!("无法从导入内容派生合法宠物 id：{id}"));
    }
    let name = if petdex_name.is_empty() {
        fallback_name.trim().chars().take(60).collect::<String>()
    } else {
        petdex_name
    };
    let name = if name.is_empty() { id.clone() } else { name };

    let meta = CustomPetMeta {
        id: id.clone(),
        name,
        format: format.to_string(),
        cols: 8,
        rows,
        frame_w,
        frame_h,
        image: format!("{SHEET_PREFIX}.{ext}"),
        states: build_default_states(8),
    };

    // 临时目录写全 → 替换 → 原子改名（与壁纸/配置的 .tmp+rename 同手法）
    let staging = root.join(format!(
        "{id}.importing-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default() & 0xffff_ffff
    ));
    fs::create_dir_all(&staging).map_err(|e| format!("创建导入临时目录失败：{e}"))?;
    let write = || -> Result<(), String> {
        let json = serde_json::to_string_pretty(&meta)
            .map_err(|e| format!("序列化宠物元信息失败：{e}"))?;
        fs::write(staging.join(PET_META_FILE), json)
            .map_err(|e| format!("写入宠物元信息失败：{e}"))?;
        fs::write(staging.join(&meta.image), sheet_bytes)
            .map_err(|e| format!("写入精灵图集失败：{e}"))?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let final_dir = root.join(&id);
    if final_dir.exists() {
        // 重复导入同名宠物 = 整体替换（旧图集扩展名可能不同，目录级替换）
        fs::remove_dir_all(&final_dir).map_err(|e| format!("替换旧宠物目录失败：{e}"))?;
    }
    fs::rename(&staging, &final_dir).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        format!("落盘宠物目录失败：{e}")
    })?;
    Ok(meta)
}

// ============================================================
// 内置形象释放（「智谱 Z 娘」随安装包分发，启动时确保库内就位）
// ============================================================

/// 库目录内某只宠物是否完整可用（pet.json 可解析且 id 匹配 + 图集存在
/// 且非空）：内置形象的体检口径，同时被「已存在跳过」与单测复用。
fn builtin_pet_ok_in(root: &Path, id: &str) -> bool {
    match load_pet_meta_in(root, id) {
        Ok(meta) => {
            meta.id == id
                && fs::metadata(root.join(id).join(&meta.image))
                    .is_ok_and(|m| m.len() > 0)
        }
        Err(_) => false,
    }
}

/// 内置宠物元信息（内嵌字节的解析形态；损坏即 panic 于编译资产，正常
/// 构建不应出现——发布前有单测兜底校验）。id 与 BUILTIN_PET_ID 一致。
pub(crate) fn builtin_pet_meta() -> CustomPetMeta {
    let text = std::str::from_utf8(BUILTIN_PET_JSON)
        .expect("内置宠物 pet.json 应为 UTF-8 文本");
    let meta: CustomPetMeta =
        serde_json::from_str(text).expect("内置宠物 pet.json 应可解析");
    assert_eq!(meta.id, BUILTIN_PET_ID, "内置宠物 id 应与常量一致");
    meta
}

/// 确保内置形象（智谱娘）在宠物库就位（真实 ~/.zbar 路径版，应用启动
/// setup 阶段调用，失败由调用方记日志不阻断启动）：
/// - 库内已存在且体检通过（pet.json 可解析且 id 匹配 + 图集非空）→
///   比对 pet.json 字节与内置版本，不一致则覆盖升级（V9 语义，见
///   [`ensure_builtin_pet_in`]），一致则跳过；图集不比对覆盖；
/// - 缺失或损坏（pet.json 解析失败/图集缺失或为空）→ 重新释放：临时
///   目录写全 → 移除旧目录 → 原子改名（与导入同手法，杜绝半截状态）。
pub fn ensure_builtin_pet() -> Result<(), String> {
    let root = pets_root()?;
    ensure_builtin_pet_in(&root)
}

/// ensure_builtin_pet 的目录显式版（单元测试复用，不依赖真实 ~/.zbar）。
/// 已存在且体检通过时的升级语义（V9 起）：
/// - **pet.json 覆盖升级**：内置宠物的状态映射（states 键）属软件管理
///   范畴，随版本演进（V9 的 thinking/walking 细分键等）需要能升级到
///   用户库——库内字节与内嵌 BUILTIN_PET_JSON 不一致时 .tmp + rename
///   原子覆盖（与 sync_theme_custom_pet 的 pet-custom.js 同手法），
///   一致则跳写（mtime 不动）。因此手改内置 pet.json 的自定义内容会被
///   升级回内置版（想自定义请导入自己的宠物）；删除保护见
///   delete_custom_pet_impl；
/// - **图集不覆盖**：sheet.webp 约 2.5MB 且不随版本变化，仅缺失/损坏
///   （体检不过 → 整目录重释）时才写盘，避免每次启动无谓写盘。
pub(crate) fn ensure_builtin_pet_in(root: &Path) -> Result<(), String> {
    fs::create_dir_all(root).map_err(|e| format!("创建宠物库目录失败：{e}"))?;
    if builtin_pet_ok_in(root, BUILTIN_PET_ID) {
        upgrade_builtin_pet_meta_in(root)?;
        return Ok(()); // 已就位且合法：仅按需升级元数据（图集不动）
    }
    let staging = root.join(format!(
        "{BUILTIN_PET_ID}.builtin-{}",
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or_default() & 0xffff_ffff
    ));
    fs::create_dir_all(&staging).map_err(|e| format!("创建内置形象临时目录失败：{e}"))?;
    let write = || -> Result<(), String> {
        fs::write(staging.join(PET_META_FILE), BUILTIN_PET_JSON)
            .map_err(|e| format!("写入内置形象元信息失败：{e}"))?;
        fs::write(staging.join(builtin_pet_meta().image), BUILTIN_PET_SHEET)
            .map_err(|e| format!("写入内置形象图集失败：{e}"))?;
        Ok(())
    };
    if let Err(e) = write() {
        let _ = fs::remove_dir_all(&staging);
        return Err(e);
    }
    let final_dir = root.join(BUILTIN_PET_ID);
    if final_dir.exists() {
        // 损坏/半截的旧目录：整体移除后由内置资产重建
        fs::remove_dir_all(&final_dir)
            .map_err(|e| format!("清理损坏的内置形象目录失败：{e}"))?;
    }
    fs::rename(&staging, &final_dir).map_err(|e| {
        let _ = fs::remove_dir_all(&staging);
        format!("落盘内置形象目录失败：{e}")
    })?;
    Ok(())
}

/// 内置形象元数据升级：库内 pet.json 字节与内嵌 BUILTIN_PET_JSON 不一致
/// 时 .tmp + rename 原子覆盖为内置版（升级语义见 ensure_builtin_pet_in
/// 的函数注释），一致则跳写（mtime 不动，启动路径零多余写盘）。仅在
/// 体检通过路径调用——损坏路径由上层整目录重释兜底。
fn upgrade_builtin_pet_meta_in(root: &Path) -> Result<(), String> {
    let dir = root.join(BUILTIN_PET_ID);
    let meta_path = dir.join(PET_META_FILE);
    if fs::read(&meta_path).is_ok_and(|b| b == BUILTIN_PET_JSON) {
        return Ok(()); // 已同版：跳写
    }
    let tmp = dir.join(format!("{PET_META_FILE}.tmp"));
    fs::write(&tmp, BUILTIN_PET_JSON)
        .map_err(|e| format!("写入内置形象元信息失败：{e}"))?;
    fs::rename(&tmp, &meta_path).map_err(|e| {
        let _ = fs::remove_file(&tmp);
        format!("升级内置形象元信息失败：{e}")
    })
}

// ============================================================
// 读取与缩略图
// ============================================================

/// 读取指定库目录下某只宠物的元信息（不存在/损坏报中文错误）
pub(crate) fn load_pet_meta_in(root: &Path, id: &str) -> Result<CustomPetMeta, String> {
    let dir = root.join(id);
    let text = fs::read_to_string(dir.join(PET_META_FILE))
        .map_err(|_| format!("自定义宠物不存在：{id}"))?;
    let meta: CustomPetMeta = serde_json::from_str(&text)
        .map_err(|e| format!("自定义宠物元信息损坏（{id}）：{e}"))?;
    Ok(normalize_meta(meta))
}

/// 读取某只宠物的完整渲染资产：meta + 图集 dataUri
pub(crate) fn build_asset_in(root: &Path, id: &str) -> Result<(CustomPetMeta, String), String> {
    let meta = load_pet_meta_in(root, id)?;
    let sheet = fs::read(root.join(id).join(&meta.image))
        .map_err(|e| format!("读取精灵图集失败（{id}）：{e}"))?;
    let ext = if meta.image.ends_with(".png") { "png" } else { "webp" };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&sheet);
    Ok((meta, format!("data:image/{ext};base64,{b64}")))
}

/// 缩略图（idle 行首帧，64×70 框内等比适配，最近邻保像素观感）：
/// 生成失败返回空串，前端以占位样式兜底。
pub(crate) fn thumb_data_uri(sheet_bytes: &[u8], meta: &CustomPetMeta) -> String {
    let img = match image::load_from_memory(sheet_bytes) {
        Ok(i) => i,
        Err(_) => return String::new(),
    };
    let fw = meta.frame_w.min(img.width());
    let fh = meta.frame_h.min(img.height());
    if fw == 0 || fh == 0 {
        return String::new();
    }
    let row = meta
        .states
        .get("idle")
        .map(|s| s.row)
        .unwrap_or(0)
        .min(img.height().saturating_sub(fh) / fh.max(1));
    let y = (row * fh).min(img.height() - fh);
    let crop = img.crop_imm(0, y, fw, fh);
    let scale = (THUMB_W as f64 / fw as f64).min(THUMB_H as f64 / fh as f64);
    let tw = ((fw as f64 * scale).round() as u32).max(1);
    let th = ((fh as f64 * scale).round() as u32).max(1);
    let small = crop.resize_exact(tw, th, image::imageops::FilterType::Nearest);
    let mut buf = Vec::new();
    if small
        .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
        .is_err()
    {
        return String::new();
    }
    format!(
        "data:image/png;base64,{}",
        base64::engine::general_purpose::STANDARD.encode(&buf)
    )
}

/// 清单项 DTO（list_custom_pets 契约，前端选择器消费）
#[derive(Debug, Clone, Serialize)]
pub struct CustomPetEntryDto {
    pub id: String,
    pub name: String,
    pub format: String,
    /// idle 行首帧缩略图（64×70 内等比，PNG dataUri；生成失败为空串）
    pub thumb: String,
    /// 是否内置形象（智谱娘）：前端据此归入「内建形象」分组且不渲染
    /// 删除按钮；delete_custom_pet 对内置 id 同样拒绝（双保险）
    pub builtin: bool,
}

/// 列出库内全部宠物（含内置智谱娘，按 id 排序；坏目录跳过不阻塞清单）
pub(crate) fn list_custom_pets_in(root: &Path) -> Vec<CustomPetEntryDto> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut out: Vec<CustomPetEntryDto> = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().to_string();
        if name.starts_with('.') || !entry.path().is_dir() || !valid_pet_id(&name) {
            continue;
        }
        let Ok(meta) = load_pet_meta_in(root, &name) else {
            continue;
        };
        let thumb = fs::read(root.join(&name).join(&meta.image))
            .map(|bytes| thumb_data_uri(&bytes, &meta))
            .unwrap_or_default();
        let builtin = name == BUILTIN_PET_ID;
        out.push(CustomPetEntryDto {
            id: meta.id,
            name: meta.name,
            format: meta.format,
            thumb,
            builtin,
        });
    }
    out.sort_by(|a, b| a.id.cmp(&b.id));
    out
}

// ============================================================
// 注入版物化（pet-custom.js）
// ============================================================

/// 物化文件内容：`window.__ZBAR_PET_CUSTOM__ = { v:1, meta, dataUri };`
/// meta 为 JSON 对象字面量（JSON 是合法 JS 表达式），dataUri 用 Rust debug
/// 字符串转义（内容仅含 ASCII base64 与前缀，无引号/反斜杠风险）。
pub(crate) fn materialize_js(meta: &CustomPetMeta, data_uri: &str) -> String {
    let meta_json = serde_json::to_string(meta).unwrap_or_else(|_| "{}".to_string());
    format!("window.__ZBAR_PET_CUSTOM__ = {{ v: 1, meta: {meta_json}, dataUri: {data_uri:?} }};\n")
}

/// 同步注入版物化文件（真实 ~/.zbar 路径版）：
/// - 皮肤主题目录不存在（未安装/已卸载）→ 无注入版可消费，直接返回；
/// - 当前 PetConfig.style 为 custom:<id> 且宠物可读 → 内容无变化跳写，
///   有变化 .tmp + rename 原子重写；
/// - 其余情形（非 custom 选中 / 宠物缺失）→ 删除残留物化文件。
/// 选中形象与重渲参数统一取 pet.json（宠物配置唯一真相源）。
pub fn sync_theme_custom_pet() -> Result<(), String> {
    let theme_dir = crate::agent_theme::store::app_dir("zcode")?;
    let root = pets_root()?;
    let pet = crate::pet::load_pet_config().clamped();
    sync_theme_custom_pet_in(&theme_dir, &root, &pet)
}

/// sync_theme_custom_pet 的目录显式版（单元测试复用）：宠物配置显式
/// 传入（选中形象与 variables.css 重渲共用一份，不依赖真实 pet.json）。
/// 物化/清除后重渲 variables.css：`--zbar-pet-asset-ver` 内容戳随之
/// 更新，注入版壳经每秒热重载读到新值即重载资产（P1-2：重复导入同 id
/// 时物化文件已变但页面无感知，旧图集滞留至重载——内容戳是变化信号）。
pub(crate) fn sync_theme_custom_pet_in(
    theme_dir: &Path,
    root: &Path,
    pet: &crate::pet::PetConfig,
) -> Result<(), String> {
    if !theme_dir.is_dir() {
        return Ok(()); // 皮肤未安装：注入版本不存在，无需物化
    }
    let target = theme_dir.join(PET_CUSTOM_JS);
    if let Some(id) = custom_style_id(&pet.style).map(str::to_string) {
        match build_asset_in(root, &id) {
            Ok((meta, data_uri)) => {
                let content = materialize_js(&meta, &data_uri);
                // 内容未变跳写（mtime 不动）；内容变化（含重复导入同 id）
                // .tmp + rename 原子重写
                if !fs::read_to_string(&target).is_ok_and(|old| old == content) {
                    let tmp = theme_dir.join(format!("{PET_CUSTOM_JS}.tmp"));
                    fs::write(&tmp, &content)
                        .map_err(|e| format!("写入 pet-custom.js 失败: {e}"))?;
                    fs::rename(&tmp, &target)
                        .map_err(|e| format!("落盘 pet-custom.js 失败: {e}"))?;
                }
            }
            Err(_) => {
                // 选中宠物已缺失（被删除等）：移除物化文件——V8 起核心无
                // 内建回退，壳读不到资产即宠物暂隐（默认智谱娘受删除保护
                // 永在库中，此路径实际只剩库目录损坏的极端情形）
                let _ = fs::remove_file(&target);
            }
        }
    } else {
        let _ = fs::remove_file(&target); // 非自定义选中：清掉残留
    }
    // 无论上面走哪条分支都重渲 variables.css（内容不变时其自身跳写）：
    // 内容戳与物化文件状态保持同步（含从旧版 variables.css 补出该变量的
    // 场景——skip-write 路径不能跳过重渲）
    crate::agent_theme::store::refresh_variables_css_in(theme_dir, pet)
}

// ============================================================
// 独立版热刷新（导入替换/删除后让悬浮窗重取资产）
// ============================================================

/// 独立悬浮窗当前选中指定自定义宠物时，重推参数事件（pet-main 收到
/// custom:* 形象会重新 invoke 获取资产并热切换）；窗口不在时 emit 无
/// 接收者，push_pet_params 内部已忽略发送结果。size 按缓存屏高换算为
/// px 推送（本函数在命令线程调用，不能查 monitor——macOS 要求主线程；
/// 与窗口实际所在屏的细微失配由下次主线程路径的同步尺寸纠正）。
fn push_pet_refresh_if_selected(app: &tauri::AppHandle, id: &str) {
    let cfg = crate::pet::load_pet_config().clamped();
    if !cfg.enabled || cfg.style != format!("{CUSTOM_STYLE_PREFIX}{id}") {
        return;
    }
    let size_px = crate::pet::pet_size_px(cfg.size, crate::pet::cached_screen_height());
    crate::pet::push_pet_params(app, &cfg, size_px);
}

// ============================================================
// Tauri 命令
// ============================================================

/// 导入自定义宠物。`src_path` 支持三种形态（Petdex 分发以两文件为主，zip 为
/// 画廊下载的传输封装）：
/// - `.zip` 包：包内提取 pet.json（可选）+ spritesheet（必需）；
/// - `pet.json`：同目录找 spritesheet.webp / .png；
/// - 裸图集（.png / .webp）：无 pet.json 时用默认映射与文件名生成。
/// 重复导入同 id 为整体替换；导入成功后同步注入版物化文件并按需热刷新
/// 独立悬浮窗。返回清单项（含缩略图）。
/// 参数名契约：前端 invoke 键为 `srcPath`（Tauri 2 按 Rust 参数名的
/// camelCase 精确匹配），与 set_agent_wallpaper(src_path) 同款惯例
/// —— 曾因 path/srcPath 不一致导致三种导入形态全部 invalid args（P0-1）。
#[tauri::command]
pub async fn import_pet(src_path: String, app: tauri::AppHandle) -> Result<CustomPetEntryDto, String> {
    tauri::async_runtime::spawn_blocking(move || import_pet_impl(&src_path, &app))
        .await
        .map_err(|e| format!("导入任务失败：{e}"))?
}

fn import_pet_impl(path: &str, app: &tauri::AppHandle) -> Result<CustomPetEntryDto, String> {
    let src = PathBuf::from(path);
    if !src.is_file() {
        return Err(format!("文件不存在：{path}"));
    }
    if src.metadata().map(|m| m.len()).unwrap_or(0) > MAX_SHEET_BYTES * 4 {
        return Err("文件过大（超过 64MB 上限）".into());
    }
    let ext = src
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    let stem = src
        .file_stem()
        .map(|s| s.to_string_lossy().to_string())
        .unwrap_or_default();
    let (pet_json_text, sheet_bytes, fallback_name) = match ext.as_str() {
        "zip" => {
            let bytes = fs::read(&src).map_err(|e| format!("读取 zip 包失败：{e}"))?;
            let (json, sheet) = extract_zip_parts(&bytes)?;
            (json, sheet, stem)
        }
        "json" => {
            let text = fs::read_to_string(&src).map_err(|e| format!("读取 pet.json 失败：{e}"))?;
            let dir = src.parent().unwrap_or(Path::new("."));
            let sheet_path = ["spritesheet.webp", "spritesheet.png", "sprite.webp", "sprite.png"]
                .iter()
                .map(|n| dir.join(n))
                .find(|p| p.is_file())
                .ok_or("pet.json 同目录下未找到精灵图集（spritesheet.webp / spritesheet.png）")?;
            // P1-1：旁路图集同样读取封顶（metadata 预检 + take 兜底，
            // 与 zip 条目同口径，防超大文件全量入内存）
            if sheet_path.metadata().map(|m| m.len()).unwrap_or(0) > MAX_READ_BYTES {
                return Err("精灵图集过大（超过 64MB 上限）".into());
            }
            let file = fs::File::open(&sheet_path)
                .map_err(|e| format!("读取精灵图集失败：{e}"))?;
            let bytes = read_capped(file, "精灵图集")?;
            (Some(text), bytes, stem)
        }
        "png" | "webp" => {
            let bytes = fs::read(&src).map_err(|e| format!("读取图集失败：{e}"))?;
            (None, bytes, stem)
        }
        _ => {
            return Err(
                "仅支持导入 zip 包、pet.json 或精灵图集（png / webp）文件".into(),
            )
        }
    };

    let root = pets_root()?;
    fs::create_dir_all(&root).map_err(|e| format!("创建宠物库目录失败：{e}"))?;
    let meta = import_pet_in(
        &root,
        pet_json_text.as_deref(),
        &sheet_bytes,
        &fallback_name,
    )?;

    // 导入后联动：注入版物化（选中该宠物时重写内容，含重复导入同 id 的
    // 内容更新——variables.css 的 --zbar-pet-asset-ver 内容戳随之变化，
    // 注入版壳据此重载资产热刷新）+ 独立版热刷新
    if let Err(e) = sync_theme_custom_pet() {
        eprintln!("[zbar-pets] 同步注入版宠物物化失败: {e}");
    }
    push_pet_refresh_if_selected(app, &meta.id);

    let thumb = thumb_data_uri(&sheet_bytes, &meta);
    let builtin = meta.id == BUILTIN_PET_ID;
    Ok(CustomPetEntryDto {
        id: meta.id,
        name: meta.name,
        format: meta.format,
        thumb,
        builtin,
    })
}

/// 列出自定义宠物清单（含 idle 首帧缩略图）
#[tauri::command]
pub async fn list_custom_pets() -> Result<Vec<CustomPetEntryDto>, String> {
    tauri::async_runtime::spawn_blocking(|| {
        let root = pets_root()?;
        Ok(list_custom_pets_in(&root))
    })
    .await
    .map_err(|e| format!("读取宠物清单任务失败：{e}"))?
}

/// 读取自定义宠物的渲染资产（独立悬浮窗 pet-main 消费：
/// style 为 custom:* 时 invoke 获取后作为 customAsset 传给 pet-core）
#[tauri::command]
pub async fn get_custom_pet_asset(id: String) -> Result<CustomPetAssetDto, String> {
    tauri::async_runtime::spawn_blocking(move || {
        if !valid_pet_id(&id) {
            return Err(format!("非法宠物 id：{id}"));
        }
        let root = pets_root()?;
        let (meta, data_uri) = build_asset_in(&root, &id)?;
        Ok(CustomPetAssetDto { meta, data_uri })
    })
    .await
    .map_err(|e| format!("读取宠物资产任务失败：{e}"))?
}

/// 自定义宠物渲染资产 DTO（get_custom_pet_asset 契约）。
/// 【P0-2 契约】必须 camelCase：前端 types.ts 与 pet-core.js 消费
/// `dataUri`（customAssetValid 据此判定资产有效），下划线形态会让
/// 独立悬浮窗的 customAsset 恒无效、静默回退内建形象。
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CustomPetAssetDto {
    pub meta: CustomPetMeta,
    pub data_uri: String,
}

/// 删除自定义宠物。内置形象（智谱娘）不可删除（返回中文错误，前端
/// 的删除按钮对 builtin 项同样不渲染——双保险）。若宠物配置（pet.json
/// 唯一真相源，注入版/悬浮窗共用）正选中该宠物，先回退默认形象（内置
/// 智谱娘——它永在库中，回退必达；落盘 + 悬浮窗热推参数 + 注入版重渲
/// variables.css 热生效），再清理注入版物化文件与库目录。
#[tauri::command]
pub async fn delete_custom_pet(id: String, app: tauri::AppHandle) -> Result<(), String> {
    tauri::async_runtime::spawn_blocking(move || delete_custom_pet_impl(&id, &app))
        .await
        .map_err(|e| format!("删除任务失败：{e}"))?
}

/// 删除前置校验（纯函数，单测直测）：id 合法性 + 内置形象保护
fn check_deletable(id: &str) -> Result<(), String> {
    if !valid_pet_id(id) {
        return Err(format!("非法宠物 id：{id}"));
    }
    if id == BUILTIN_PET_ID {
        return Err("内置形象不可删除".to_string());
    }
    Ok(())
}

fn delete_custom_pet_impl(id: &str, app: &tauri::AppHandle) -> Result<(), String> {
    check_deletable(id)?;
    let custom_style = format!("{CUSTOM_STYLE_PREFIX}{id}");

    // 选中回退：重置为默认形象（内置智谱娘，永在库中）并落盘（悬浮窗
    // 热推参数——size 按缓存屏高换算 px，同 push_pet_refresh_if_selected
    // 的线程口径；注入版经下方 sync_theme_custom_pet 重渲 variables.css
    // 热生效）
    let mut pet_cfg = crate::pet::load_pet_config();
    if pet_cfg.style == custom_style {
        pet_cfg.style = crate::pet::DEFAULT_PET_STYLE.to_string();
        crate::pet::save_pet_config(&pet_cfg)?;
        let cfg = pet_cfg.clamped();
        let size_px =
            crate::pet::pet_size_px(cfg.size, crate::pet::cached_screen_height());
        crate::pet::push_pet_params(app, &cfg, size_px);
    }

    // 清理：库目录 + 注入版物化残留
    let root = pets_root()?;
    let dir = root.join(id);
    if dir.exists() {
        fs::remove_dir_all(&dir).map_err(|e| format!("删除宠物目录失败: {e}"))?;
    }
    sync_theme_custom_pet()?;
    Ok(())
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write as _; // ZipWriter 写入条目内容

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-pets-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    /// 生成一张最小合法 PNG 图集（width×height 纯色 RGBA）：
    /// 用 image crate 编码，测试不依赖外部素材。
    fn make_sheet_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([200, 120, 60, 255]));
        let dyn_img = image::DynamicImage::ImageRgba8(img);
        let mut buf = Vec::new();
        dyn_img
            .write_to(&mut Cursor::new(&mut buf), image::ImageFormat::Png)
            .unwrap();
        buf
    }

    #[test]
    fn slug派生_拉丁名直通_非拉丁回退哈希() {
        assert_eq!(derive_pet_id(&["Boba Tea"]), "boba-tea");
        assert_eq!(derive_pet_id(&["  ", "My Pet"]), "my-pet");
        // 中文/emoji 无法 slug 化 → fnv 哈希兜底（确定性）
        let a = derive_pet_id(&["泡泡龙"]);
        assert!(a.starts_with("pet-") && a.len() == 12, "{a}");
        assert_eq!(a, derive_pet_id(&["泡泡龙"]), "同输入应派生同 id");
        // 超长输入截断到 40 字符
        let long = derive_pet_id(&["x".repeat(80).as_str()]);
        assert_eq!(long.len(), 40);
    }

    #[test]
    fn 网格判定_行列整除与版本裁定() {
        // 标准 v1 / v2 尺寸
        assert_eq!(grid_of(1536, 1872, None).unwrap(), (9, "petdex-v1"));
        assert_eq!(grid_of(1536, 2288, None).unwrap(), (11, "petdex-v2"));
        // pet.json 声明优先（v2 声明 + v2 尺寸）
        assert_eq!(grid_of(1536, 2288, Some(2)).unwrap(), (11, "petdex-v2"));
        // 声明与图集不符：以能整除者为准（声明 v2 但 v1 尺寸）
        assert_eq!(grid_of(1536, 1872, Some(2)).unwrap(), (9, "petdex-v1"));
        // clean scale（2 倍）
        assert_eq!(grid_of(3072, 3744, None).unwrap(), (9, "petdex-v1"));
        assert_eq!(grid_of(3072, 4576, Some(2)).unwrap(), (11, "petdex-v2"));
        // 宽度不整除 8 列（1004 = 8×125.5）
        assert!(grid_of(1004, 1872, None).is_err());
        // 高度既不整除 9 也不整除 11
        assert!(grid_of(1536, 1000, None).is_err());
    }

    #[test]
    fn 导入_裸图集与petjson两形态() {
        let root = test_dir("import");
        // 裸图集：默认映射 + 文件名生成 id/name
        let meta = import_pet_in(&root, None, &make_sheet_png(1536, 1872), "My Pet").unwrap();
        assert_eq!(meta.id, "my-pet");
        assert_eq!(meta.name, "My Pet");
        assert_eq!(meta.format, "petdex-v1");
        assert_eq!((meta.cols, meta.rows), (8, 9));
        assert_eq!((meta.frame_w, meta.frame_h), (192, 208));
        assert_eq!(meta.image, "sheet.png");
        // 五状态映射（Petdex 行号契约）
        assert_eq!(meta.states["sleeping"].row, 6);
        assert_eq!(meta.states["idle"].row, 0);
        assert_eq!(meta.states["working"].row, 8);
        assert_eq!(meta.states["typing"].row, 7);
        assert_eq!(meta.states["celebrating"].row, 4);
        // V6 新状态映射：tool_running → 行 1 running-right（8 帧）、
        // failed → 行 5 failed（8 帧），帧数以 petdex 官方 pet-states.ts 为准
        assert_eq!(meta.states["tool_running"].row, 1);
        assert_eq!(meta.states["tool_running"].frames, 8);
        assert_eq!(meta.states["failed"].row, 5);
        assert_eq!(meta.states["failed"].frames, 8);
        // frameMs 默认沿用内建节奏（typing 三档数组）
        assert_eq!(meta.states["sleeping"].frame_ms, serde_json::json!(800));
        assert_eq!(
            meta.states["typing"].frame_ms,
            serde_json::json!([220, 150, 95])
        );
        // 落盘读回一致
        assert_eq!(load_pet_meta_in(&root, "my-pet").unwrap(), meta);

        // pet.json 形态：id/displayName/spriteVersionNumber 消费
        let json = r#"{
            "id": "Sakura Moon",
            "displayName": "樱花月",
            "description": "test",
            "spriteVersionNumber": 2
        }"#;
        let meta2 = import_pet_in(&root, Some(json), &make_sheet_png(1536, 2288), "ignored").unwrap();
        assert_eq!(meta2.id, "sakura-moon");
        assert_eq!(meta2.name, "樱花月");
        assert_eq!(meta2.format, "petdex-v2");
        assert_eq!(meta2.rows, 11);
        assert_eq!(meta2.frame_h, 208);

        // 重复导入同 id = 整体替换（v1 → v2 尺寸替换后旧内容不残留）
        let json2 = r#"{"id":"my-pet","displayName":"Replaced"}"#;
        let meta3 = import_pet_in(&root, Some(json2), &make_sheet_png(1536, 2288), "x").unwrap();
        assert_eq!(meta3.id, "my-pet");
        assert_eq!(meta3.format, "petdex-v2");
        assert!(root.join("my-pet").join("sheet.png").is_file());
        let list = list_custom_pets_in(&root);
        assert_eq!(list.len(), 2, "替换不应产生重复目录：{list:?}");

        // 非法 spriteVersionNumber 报中文错误
        let bad = r#"{"id":"x","spriteVersionNumber":3}"#;
        let err = import_pet_in(&root, Some(bad), &make_sheet_png(1536, 1872), "x").unwrap_err();
        assert!(err.contains("spriteVersionNumber"), "{err}");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 导入_坏图集与坏json报中文错误() {
        let root = test_dir("import-invalid");
        // 无法识别的格式
        let err = import_pet_in(&root, None, b"not an image", "x").unwrap_err();
        assert!(err.contains("PNG") || err.contains("识别"), "{err}");
        // 高度既不整除 9 也不整除 11（1000 = 9×111+1 = 11×90+10）
        let err = import_pet_in(&root, None, &make_sheet_png(1536, 1000), "x").unwrap_err();
        assert!(err.contains("整除"), "{err}");
        // 坏 JSON
        let err = import_pet_in(&root, Some("{broken"), &make_sheet_png(1536, 1872), "x").unwrap_err();
        assert!(err.contains("JSON"), "{err}");
        // 空图集
        let err = import_pet_in(&root, None, &[], "x").unwrap_err();
        assert!(err.contains("为空"), "{err}");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn meta收敛_用户手改坏值回安全边界() {
        let mut meta = CustomPetMeta {
            id: "t".into(),
            name: "t".into(),
            format: "petdex-v1".into(),
            cols: 0,
            rows: 0,
            frame_w: 0,
            frame_h: 0,
            image: "  ".into(),
            states: BTreeMap::new(),
        };
        meta = normalize_meta(meta);
        assert_eq!((meta.cols, meta.rows, meta.frame_w, meta.frame_h), (8, 9, 192, 208));
        assert_eq!(meta.image, "sheet.webp");
        // 七状态齐备（V6：五状态 + tool_running/failed）
        for key in [
            "sleeping",
            "idle",
            "working",
            "typing",
            "celebrating",
            "tool_running",
            "failed",
        ] {
            assert!(meta.states.contains_key(key), "缺状态 {key}");
        }
        // 手改坏值收敛：行越界、帧数 0、frameMs 非法
        let mut m2 = meta.clone();
        m2.states.insert(
            "sleeping".into(),
            CustomPetStateDef {
                row: 99,
                frames: 0,
                frame_ms: serde_json::json!("fast"),
            },
        );
        m2.states.insert(
            "typing".into(),
            CustomPetStateDef {
                row: 1,
                frames: 3,
                frame_ms: serde_json::json!([-1, 0, "x"]),
            },
        );
        let fixed = normalize_meta(m2);
        assert_eq!(fixed.states["sleeping"].row, 8, "行应夹进行数上限");
        assert_eq!(fixed.states["sleeping"].frames, 1, "帧数下限 1");
        assert_eq!(fixed.states["sleeping"].frame_ms, serde_json::json!(800));
        assert_eq!(
            fixed.states["typing"].frame_ms,
            serde_json::json!([220, 150, 95]),
            "非法 typing 数组应回三档默认"
        );
    }

    #[test]
    fn 旧五状态petjson_读取时补齐新状态默认行() {
        // V6 兼容：V6 前导入的 pet.json 只有五状态键，读取（normalize_meta）
        // 时缺 tool_running/failed 键用默认行补齐——老数据平滑升级，无需
        // 重新导入；既有五状态的用户自定义值原样保留
        let mut meta = CustomPetMeta {
            id: "legacy".into(),
            name: "Legacy".into(),
            format: "petdex-v1".into(),
            cols: 8,
            rows: 9,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.webp".into(),
            states: BTreeMap::from([
                (
                    "sleeping".into(),
                    CustomPetStateDef { row: 6, frames: 6, frame_ms: serde_json::json!(800) },
                ),
                (
                    "idle".into(),
                    CustomPetStateDef { row: 0, frames: 6, frame_ms: serde_json::json!(450) },
                ),
                (
                    "working".into(),
                    CustomPetStateDef { row: 8, frames: 6, frame_ms: serde_json::json!(300) },
                ),
                (
                    "typing".into(),
                    CustomPetStateDef {
                        row: 7,
                        frames: 6,
                        frame_ms: serde_json::json!([220, 150, 95]),
                    },
                ),
                // 用户手改过 celebrating 行（如换成 waving 行 3），补齐时不得覆盖
                (
                    "celebrating".into(),
                    CustomPetStateDef { row: 3, frames: 4, frame_ms: serde_json::json!(400) },
                ),
            ]),
        };
        meta = normalize_meta(meta);
        // 既有键原样保留（含用户自定义的 celebrating）
        assert_eq!(meta.states["celebrating"].row, 3, "既有键的用户自定义值不应被覆盖");
        assert_eq!(meta.states["typing"].frame_ms, serde_json::json!([220, 150, 95]));
        // 新键补默认行（Petdex 官方行号与帧数）
        assert_eq!(meta.states["tool_running"].row, 1);
        assert_eq!(meta.states["tool_running"].frames, 8);
        assert_eq!(meta.states["tool_running"].frame_ms, serde_json::json!(150));
        assert_eq!(meta.states["failed"].row, 5);
        assert_eq!(meta.states["failed"].frames, 8);
        assert_eq!(meta.states["failed"].frame_ms, serde_json::json!(300));
        // 空状态（极端旧数据）同样七状态齐备
        let empty = CustomPetMeta {
            id: "e".into(),
            name: "e".into(),
            format: "petdex-v1".into(),
            cols: 8,
            rows: 9,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.webp".into(),
            states: BTreeMap::new(),
        };
        let fixed = normalize_meta(empty);
        assert_eq!(fixed.states.len(), 7, "七状态应全部补齐");
    }

    #[test]
    fn 物化_内容格式与原子写跳写() {
        let theme_dir = test_dir("materialize");
        // 皮肤未安装（目录不存在）→ no-op
        sync_theme_custom_pet_in(
            &theme_dir.parent().unwrap().join("no-such"),
            &theme_dir,
            &crate::pet::PetConfig::default(),
        )
        .unwrap();
        assert!(!theme_dir.join(PET_CUSTOM_JS).exists());

        // 直接测物化内容格式（meta + dataUri）
        let meta = CustomPetMeta {
            id: "boba".into(),
            name: "Boba".into(),
            format: "petdex-v1".into(),
            cols: 8,
            rows: 9,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.webp".into(),
            states: build_default_states(8),
        };
        let js = materialize_js(&meta, "data:image/webp;base64,QUJD");
        assert!(
            js.starts_with("window.__ZBAR_PET_CUSTOM__ = { v: 1, meta: {"),
            "{js}"
        );
        assert!(js.contains("\"id\":\"boba\""), "{js}");
        assert!(js.contains("\"frameW\":192"), "{js}");
        assert!(js.contains("\"frameMs\":800"), "{js}");
        assert!(js.ends_with("dataUri: \"data:image/webp;base64,QUJD\" };\n"), "{js}");
        let _ = fs::remove_dir_all(&theme_dir);
    }

    #[test]
    fn 物化_端到端_选中写出_切走清除_缺失清除_跳写() {
        let root = test_dir("e2e-pets");
        let theme_dir = test_dir("e2e-theme");
        // 导入一只真实宠物（图集生成 → 内部格式落盘）
        import_pet_in(&root, None, &make_sheet_png(1536, 1872), "Boba").unwrap();

        // 未选中自定义（非 custom 前缀的 style 值，如迁移前的旧内建残留）
        // → 无物化文件
        let builtin_cfg = crate::pet::PetConfig {
            style: "legacy-value".into(),
            ..crate::pet::PetConfig::default()
        };
        sync_theme_custom_pet_in(&theme_dir, &root, &builtin_cfg).unwrap();
        assert!(!theme_dir.join(PET_CUSTOM_JS).exists());

        // 选中 custom:boba → 物化完整资产（meta + base64 图集）
        let custom_cfg = crate::pet::PetConfig {
            style: "custom:boba".into(),
            ..crate::pet::PetConfig::default()
        };
        sync_theme_custom_pet_in(&theme_dir, &root, &custom_cfg).unwrap();
        let js = fs::read_to_string(theme_dir.join(PET_CUSTOM_JS)).unwrap();
        assert!(js.starts_with("window.__ZBAR_PET_CUSTOM__ = { v: 1,"), "{js}");
        assert!(js.contains("\"id\":\"boba\""), "{js}");
        assert!(js.contains("\"states\":{"), "{js}");
        assert!(
            js.contains("data:image/png;base64,"),
            "图集应转为 dataUri：{}",
            &js[js.len().saturating_sub(80)..]
        );
        // P1-2：物化后 variables.css 重渲并透出内容戳（非空 16 位十六进制）
        let css = fs::read_to_string(theme_dir.join(crate::agent_theme::store::VARIABLES_CSS))
            .unwrap();
        assert!(
            css.contains("--zbar-pet-asset-ver: ") && !css.contains("--zbar-pet-asset-ver: ;"),
            "物化存在时内容戳应为非空值：{css}"
        );
        let stamp1: String = css
            .split("--zbar-pet-asset-ver: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string();
        assert_eq!(stamp1.len(), 16, "FNV-1a 64 位十六进制：{stamp1}");

        // 幂等跳写：内容未变不重写（mtime 不变）
        let path = theme_dir.join(PET_CUSTOM_JS);
        let mtime = fs::metadata(&path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        sync_theme_custom_pet_in(&theme_dir, &root, &custom_cfg).unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().modified().unwrap(),
            mtime,
            "内容未变时不应重写 pet-custom.js"
        );

        // P1-2：重复导入同 id（同名、图集换成 2 倍 clean scale）→ 物化
        // 重写 + 内容戳变化
        import_pet_in(&root, None, &make_sheet_png(3072, 1872), "Boba").unwrap();
        sync_theme_custom_pet_in(&theme_dir, &root, &custom_cfg).unwrap();
        let css2 = fs::read_to_string(theme_dir.join(crate::agent_theme::store::VARIABLES_CSS))
            .unwrap();
        let stamp2: String = css2
            .split("--zbar-pet-asset-ver: ")
            .nth(1)
            .unwrap()
            .split(';')
            .next()
            .unwrap()
            .trim()
            .to_string();
        assert_ne!(stamp1, stamp2, "重复导入后内容戳应变化（壳据此热刷新）");

        // 换选中非自定义形象 → 物化文件被清除，内容戳清空（壳不再持有旧资产）
        sync_theme_custom_pet_in(&theme_dir, &root, &builtin_cfg).unwrap();
        assert!(!theme_dir.join(PET_CUSTOM_JS).exists());
        let css3 = fs::read_to_string(theme_dir.join(crate::agent_theme::store::VARIABLES_CSS))
            .unwrap();
        assert!(
            css3.contains("--zbar-pet-asset-ver: ;"),
            "物化清除后内容戳应为空：{css3}"
        );

        // 选中已删除的宠物 → 同样清除（V8 起核心无内建回退，壳读不到
        // 资产即宠物暂隐，资产就位后热切换恢复）
        let gone_cfg = crate::pet::PetConfig {
            style: "custom:gone".into(),
            ..crate::pet::PetConfig::default()
        };
        sync_theme_custom_pet_in(&theme_dir, &root, &gone_cfg).unwrap();
        assert!(!theme_dir.join(PET_CUSTOM_JS).exists());

        // 资产读回（独立版 get_custom_pet_asset 的核心路径）
        let (meta, data_uri) = build_asset_in(&root, "boba").unwrap();
        assert_eq!(meta.id, "boba");
        assert_eq!(meta.states["typing"].frame_ms, serde_json::json!([220, 150, 95]));
        assert!(data_uri.starts_with("data:image/png;base64,"));

        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&theme_dir);
    }

    #[test]
    fn 清单_坏目录跳过并排序() {
        let root = test_dir("list");
        import_pet_in(&root, None, &make_sheet_png(1536, 1872), "Beta").unwrap();
        import_pet_in(&root, None, &make_sheet_png(1536, 1872), "Alpha").unwrap();
        // 坏目录（无 pet.json）与非法目录名（大写/点开头）跳过
        fs::create_dir_all(root.join("broken")).unwrap();
        fs::create_dir_all(root.join("UpperCase")).unwrap();
        fs::create_dir_all(root.join(".hidden")).unwrap();
        let list = list_custom_pets_in(&root);
        let ids: Vec<&str> = list.iter().map(|e| e.id.as_str()).collect();
        assert_eq!(ids, vec!["alpha", "beta"], "{ids:?}");
        // 导入宠物非内置（builtin 标志为 false，前端据此渲染删除按钮）
        assert!(list.iter().all(|e| !e.builtin), "{list:?}");
        // 缩略图生成成功（PNG dataUri）
        assert!(list[0].thumb.starts_with("data:image/png;base64,"), "{:?}",
            list[0].thumb.chars().take(40).collect::<String>());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 缩略图_idle行首帧尺寸约束() {
        let root = test_dir("thumb");
        let meta = import_pet_in(&root, None, &make_sheet_png(1536, 1872), "T").unwrap();
        let sheet = fs::read(root.join("t").join(&meta.image)).unwrap();
        let thumb = thumb_data_uri(&sheet, &meta);
        assert!(thumb.starts_with("data:image/png;base64,"));
        // 解码校验尺寸：192×208 等比适配 64×70 → 64×69
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(thumb.trim_start_matches("data:image/png;base64,"))
            .unwrap();
        let dim = image::ImageReader::new(Cursor::new(&bytes))
            .with_guessed_format()
            .unwrap()
            .into_dimensions()
            .unwrap();
        assert_eq!(dim, (64, 69), "192×208 按宽适配 64 应得 64×69");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 自定义形象id解析与校验() {
        assert_eq!(custom_style_id("custom:boba"), Some("boba"));
        assert_eq!(custom_style_id("cat"), None);
        assert_eq!(custom_style_id("custom:"), None, "空 id 非法");
        assert_eq!(custom_style_id("custom:../etc"), None, "路径遍历非法");
        assert_eq!(custom_style_id("custom:UpperCase"), None, "大写非法");
        assert!(valid_pet_id("boba-01"));
        assert!(!valid_pet_id(""));
        assert!(!valid_pet_id("-lead"));
        assert!(!valid_pet_id("a b"));
    }

    #[test]
    fn zip解包_根目录与单层包裹() {
        // 用 zip crate 在内存构造两个包型
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("pet.json", opt).unwrap();
            w.write_all(br#"{"id":"zip-pet","displayName":"Zip Pet"}"#).unwrap();
            w.start_file("spritesheet.png", opt).unwrap();
            w.write_all(&make_sheet_png(1536, 1872)).unwrap();
            w.finish().unwrap();
        }
        let (json, sheet) = extract_zip_parts(&buf).unwrap();
        assert_eq!(json.as_deref(), Some(r#"{"id":"zip-pet","displayName":"Zip Pet"}"#));
        assert!(!sheet.is_empty());

        // 单层目录包裹（网站有时打包为 <slug>/pet.json）
        let mut buf2 = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf2));
            let opt = zip::write::SimpleFileOptions::default();
            w.start_file("boba/pet.json", opt).unwrap();
            w.write_all(br#"{"id":"nested"}"#).unwrap();
            w.start_file("boba/spritesheet.webp", opt).unwrap();
            w.write_all(b"fakewebp").unwrap();
            w.finish().unwrap();
        }
        let (json2, sheet2) = extract_zip_parts(&buf2).unwrap();
        assert_eq!(json2.as_deref(), Some(r#"{"id":"nested"}"#));
        assert_eq!(sheet2, b"fakewebp".to_vec());

        // 缺图集的包报中文错误
        let mut buf3 = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf3));
            w.start_file("pet.json", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"{}").unwrap();
            w.finish().unwrap();
        }
        let err = extract_zip_parts(&buf3).unwrap_err();
        assert!(err.contains("精灵图集"), "{err}");
    }

    #[test]
    fn zip解包_解压炸弹被读取封顶拦截() {
        // P1-1：DEFLATE 全零条目压缩后极小，解压可膨胀数 GB——必须在
        // 读取封顶处被拦截（而非全量读入后才被体积校验拒绝）
        let mut buf = Vec::new();
        {
            let mut w = zip::ZipWriter::new(Cursor::new(&mut buf));
            let opt = zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Deflated);
            w.start_file("spritesheet.png", opt).unwrap();
            // 65MB 全零（> 64MB 上限）
            w.write_all(&vec![0u8; 65 * 1024 * 1024]).unwrap();
            w.finish().unwrap();
        }
        assert!(buf.len() < 1024 * 1024, "压缩后应很小（炸弹形态）：{}", buf.len());
        let err = extract_zip_parts(&buf).unwrap_err();
        assert!(err.contains("过大") && err.contains("64MB"), "{err}");
    }

    #[test]
    fn 前端契约_参数名与DTO序列化形状() {
        // P0-1：Tauri 2 按 Rust 参数名的 camelCase 精确匹配 invoke 键，
        // 前端 srcPath ↔ Rust src_path 必须一字不差（曾因 path/srcPath
        // 不一致导致三种导入形态全部 invalid args）。以源码扫描锁契约。
        let api = fs::read_to_string("../src/api.ts")
            .expect("应能读取 src/api.ts（cargo test 以 src-tauri 为 CWD，仓库根为其上一级）");
        assert!(
            api.contains("invoke<CustomPetEntry>(\"import_pet\", { srcPath })"),
            "前端 import_pet 必须以 srcPath 为键：见 src/api.ts"
        );
        // 其余命令为单词参数（id），无 camelCase 折损；get_custom_pet_asset
        // 由独立窗口宿主（pet-main.ts）直接 invoke
        assert!(api.contains("invoke(\"delete_custom_pet\", { id })"), "{api}");
        let pet_main = fs::read_to_string("../src/pet-main.ts")
            .expect("应能读取 src/pet-main.ts");
        assert!(
            pet_main.contains("invoke<CustomPetAsset>(\"get_custom_pet_asset\", { id })"),
            "pet-main 必须以 id 为键 invoke get_custom_pet_asset：{pet_main}"
        );

        // P0-2：资产 DTO 必须以 camelCase 序列化（前端 types.ts 与
        // pet-core.js 的 customAssetValid 消费 dataUri——下划线形态会让
        // 独立悬浮窗 customAsset 恒无效、静默回退内建形象）。单测此前只
        // 覆盖 build_asset_in 的数据结构，此处补 IPC 序列化边界。
        let meta = CustomPetMeta {
            id: "boba".into(),
            name: "Boba".into(),
            format: "petdex-v1".into(),
            cols: 8,
            rows: 9,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.png".into(),
            states: build_default_states(8),
        };
        let dto = CustomPetAssetDto {
            meta: meta.clone(),
            data_uri: "data:image/png;base64,QUJD".into(),
        };
        let json = serde_json::to_string(&dto).unwrap();
        assert!(
            json.contains("\"dataUri\":\"data:image/png;base64,QUJD\""),
            "dataUri 应为 camelCase 键：{json}"
        );
        assert!(!json.contains("data_uri"), "下划线键不应出现：{json}");
        // meta 的 camelCase 形状（frameW/frameH，核心 customAssetValid 消费）
        assert!(json.contains("\"frameW\":192"), "{json}");
        assert!(json.contains("\"frameH\":208"), "{json}");
        assert!(json.contains("\"frameMs\":800"), "{json}");
        // 清单项 DTO 形状（thumb + builtin 键）
        let entry = CustomPetEntryDto {
            id: "boba".into(),
            name: "Boba".into(),
            format: "petdex-v1".into(),
            thumb: "data:image/png;base64,QQ==".into(),
            builtin: false,
        };
        let ej = serde_json::to_string(&entry).unwrap();
        assert!(ej.contains("\"thumb\":\"data:image/png;base64,QQ==\""), "{ej}");
        assert!(ej.contains("\"builtin\":false"), "{ej}");
    }

    // ===== 内置形象（智谱娘）=====

    #[test]
    fn 内置资产_编译期内嵌字节为合法petdex() {
        // 内嵌资产发布前体检：pet.json 可解析且 id/网格与图集字节一致
        //（校验复用导入链路的 sniff_sheet/grid_of，与用户导入同口径）
        let text = std::str::from_utf8(BUILTIN_PET_JSON).expect("应为 UTF-8");
        let meta: CustomPetMeta = serde_json::from_str(text).expect("pet.json 应可解析");
        assert_eq!(meta.id, BUILTIN_PET_ID);
        assert_eq!(meta.format, "petdex-v2");
        let (ext, w, h) = sniff_sheet(BUILTIN_PET_SHEET).expect("内嵌图集应可嗅探");
        assert_eq!(ext, "webp");
        let version = if meta.format == "petdex-v2" { Some(2) } else { Some(1) };
        let (rows, fmt) = grid_of(w, h, version).expect("内嵌图集网格应合法");
        assert_eq!(fmt, meta.format);
        assert_eq!(meta.rows, rows, "pet.json 行数应与图集一致");
        assert_eq!(meta.frame_w, w / 8);
        assert_eq!(meta.frame_h, h / rows);
        assert_eq!(meta.image, "sheet.webp", "释放时的图集文件名应与之对应");
    }

    #[test]
    fn 内置形象_释放_幂等与损坏重释() {
        let root = test_dir("builtin");
        // 首次：释放完整目录（pet.json + 图集）
        ensure_builtin_pet_in(&root).unwrap();
        assert!(builtin_pet_ok_in(&root, BUILTIN_PET_ID));
        let meta = load_pet_meta_in(&root, BUILTIN_PET_ID).unwrap();
        assert_eq!(meta.id, BUILTIN_PET_ID);
        assert_eq!(meta.name, "智谱 Z 娘");
        let sheet = fs::read(root.join(BUILTIN_PET_ID).join(&meta.image)).unwrap();
        assert_eq!(sheet, BUILTIN_PET_SHEET, "释放的图集应与内嵌字节一致");
        assert_eq!(
            fs::read(root.join(BUILTIN_PET_ID).join(PET_META_FILE)).unwrap(),
            BUILTIN_PET_JSON
        );

        // 元数据升级（V9 语义）：体检通过但 pet.json 字节与内置不一致
        //（此处模拟手改显示名）→ 覆盖为内置版（状态映射属软件管理范畴，
        // 想自定义请导入自己的宠物）；图集不比对覆盖（mtime 不变）、
        // 无 staging 残留
        let sheet_path = root.join(BUILTIN_PET_ID).join(&meta.image);
        let sheet_mtime = fs::metadata(&sheet_path).unwrap().modified().unwrap();
        let meta_path = root.join(BUILTIN_PET_ID).join(PET_META_FILE);
        fs::write(&meta_path, text_with_name(&meta, "我的智谱娘")).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        ensure_builtin_pet_in(&root).unwrap();
        assert_eq!(
            fs::read(&meta_path).unwrap(),
            BUILTIN_PET_JSON,
            "内置 pet.json 内容不一致时应覆盖升级为内置版"
        );
        let upgraded = load_pet_meta_in(&root, BUILTIN_PET_ID).unwrap();
        assert_eq!(upgraded.name, "智谱 Z 娘", "手改的显示名应随升级回内置版");
        assert_eq!(
            fs::metadata(&sheet_path).unwrap().modified().unwrap(),
            sheet_mtime,
            "体检通过时图集不应被重写（约 2.5MB，仅缺失/损坏才释放）"
        );
        assert!(
            !fs::read_dir(&root).unwrap().flatten().any(|e| e
                .file_name()
                .to_string_lossy()
                .contains(".builtin-")),
            "升级路径不应残留 staging 目录"
        );
        assert!(
            !root.join(BUILTIN_PET_ID).join("pet.json.tmp").exists(),
            "升级成功后不应残留 .tmp 文件"
        );

        // 幂等：内容已同版 → 跳写（pet.json mtime 不动）
        let meta_mtime = fs::metadata(&meta_path).unwrap().modified().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(50));
        ensure_builtin_pet_in(&root).unwrap();
        assert_eq!(
            fs::metadata(&meta_path).unwrap().modified().unwrap(),
            meta_mtime,
            "内容已同版时不应重写 pet.json"
        );

        // 损坏（图集被清空）→ 重释为内置内容
        fs::write(root.join(BUILTIN_PET_ID).join(&meta.image), b"").unwrap();
        ensure_builtin_pet_in(&root).unwrap();
        assert!(builtin_pet_ok_in(&root, BUILTIN_PET_ID));
        assert_eq!(
            fs::read(root.join(BUILTIN_PET_ID).join(&meta.image)).unwrap(),
            BUILTIN_PET_SHEET,
            "损坏后应重释为内置图集"
        );

        // pet.json 损坏（不可解析）→ 同样重释
        fs::write(root.join(BUILTIN_PET_ID).join(PET_META_FILE), "{broken").unwrap();
        ensure_builtin_pet_in(&root).unwrap();
        assert!(builtin_pet_ok_in(&root, BUILTIN_PET_ID));

        // 清单含内置项且带 builtin 标志（前端据此归内建分组、禁删除）
        let list = list_custom_pets_in(&root);
        let builtin = list.iter().find(|e| e.id == BUILTIN_PET_ID).expect("内置项应在清单");
        assert!(builtin.builtin);
        assert!(builtin.thumb.starts_with("data:image/png;base64,"), "缩略图应生成");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn 内置元数据升级_旧版五键覆盖为新版() {
        // V8 → V9 升级路径：用户库里是旧版 pet.json（无 thinking/walking
        // 细分键、体检通过），启动 ensure 后应覆盖为新版——细分映射无需
        // 用户重新导入即可生效；图集字节保持不动
        let root = test_dir("builtin-upgrade");
        ensure_builtin_pet_in(&root).unwrap();
        let sheet_path = root
            .join(BUILTIN_PET_ID)
            .join(builtin_pet_meta().image);
        let sheet_before = fs::read(&sheet_path).unwrap();

        // 构造 V8 形态：内置 JSON 去掉细分键（旧版 states 形态）
        let mut old: serde_json::Value = serde_json::from_slice(BUILTIN_PET_JSON).unwrap();
        let states = old["states"].as_object_mut().unwrap();
        states.remove("thinking");
        states.remove("walking");
        fs::write(
            root.join(BUILTIN_PET_ID).join(PET_META_FILE),
            serde_json::to_string_pretty(&old).unwrap(),
        )
        .unwrap();

        ensure_builtin_pet_in(&root).unwrap();
        assert_eq!(
            fs::read(root.join(BUILTIN_PET_ID).join(PET_META_FILE)).unwrap(),
            BUILTIN_PET_JSON,
            "旧版 pet.json 应升级覆盖为内置新版"
        );
        let meta = load_pet_meta_in(&root, BUILTIN_PET_ID).unwrap();
        assert_eq!(meta.states["thinking"].row, 9, "细分键应随升级可读");
        assert_eq!(meta.states["thinking"].frames, 8);
        assert_eq!(meta.states["walking"].row, 8);
        assert_eq!(fs::read(&sheet_path).unwrap(), sheet_before, "图集不应被动");
        assert!(builtin_pet_ok_in(&root, BUILTIN_PET_ID));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn normalize_细分键直读_缺键不补() {
        // V9：pet.json 含 thinking/walking 键时直读保留（不覆盖已有键，
        // 坏值照常收敛）；缺键时不补默认行（通用 v2 宠物的行语义由作者
        // 自命名，没有可靠默认——渲染端 CUSTOM_STATE_FALLBACK 回退
        // working 帧，老宠物行为与 V8 一致）
        let base = CustomPetMeta {
            id: "t9".into(),
            name: "T9".into(),
            format: "petdex-v2".into(),
            cols: 8,
            rows: 11,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.webp".into(),
            states: BTreeMap::from([
                (
                    "thinking".into(),
                    CustomPetStateDef { row: 9, frames: 8, frame_ms: serde_json::json!(400) },
                ),
                (
                    "walking".into(),
                    CustomPetStateDef { row: 8, frames: 6, frame_ms: serde_json::json!(300) },
                ),
            ]),
        };
        let fixed = normalize_meta(base.clone());
        assert_eq!(fixed.states["thinking"].row, 9, "既有细分键应直读保留");
        assert_eq!(fixed.states["thinking"].frames, 8);
        assert_eq!(fixed.states["thinking"].frame_ms, serde_json::json!(400));
        assert_eq!(fixed.states["walking"].row, 8);
        assert_eq!(fixed.states["walking"].frame_ms, serde_json::json!(300));

        // 坏值收敛：row 越界夹进行数、非法 frameMs 回 400
        let mut bad = base;
        bad.states.insert(
            "thinking".into(),
            CustomPetStateDef { row: 99, frames: 0, frame_ms: serde_json::json!(0) },
        );
        let fixed_bad = normalize_meta(bad);
        assert_eq!(fixed_bad.states["thinking"].row, 10, "行应夹进行数上限（11 行）");
        assert_eq!(fixed_bad.states["thinking"].frames, 1);
        assert_eq!(fixed_bad.states["thinking"].frame_ms, serde_json::json!(400));

        // 缺键不补：七键恒在 + 无 thinking/walking（合计 7 个）
        let legacy = CustomPetMeta {
            id: "l".into(),
            name: "L".into(),
            format: "petdex-v1".into(),
            cols: 8,
            rows: 9,
            frame_w: 192,
            frame_h: 208,
            image: "sheet.webp".into(),
            states: BTreeMap::new(),
        };
        let nofill = normalize_meta(legacy);
        assert_eq!(nofill.states.len(), 7, "缺细分键时不应补默认行");
        assert!(!nofill.states.contains_key("thinking"));
        assert!(!nofill.states.contains_key("walking"));
    }

    /// 构造仅替换显示名的 pet.json 文本（模拟用户手改）
    fn text_with_name(meta: &CustomPetMeta, name: &str) -> String {
        let mut m = meta.clone();
        m.name = name.to_string();
        serde_json::to_string_pretty(&m).unwrap()
    }

    #[test]
    fn 内置形象_删除被拒绝() {
        assert!(check_deletable("boba").is_ok());
        let err = check_deletable(BUILTIN_PET_ID).unwrap_err();
        assert!(err.contains("内置形象不可删除"), "{err}");
        // 非法 id 照旧拒绝（路径遍历防护不受内置保护影响）
        assert!(check_deletable("../etc").is_err());
        assert!(check_deletable("").is_err());
    }
}

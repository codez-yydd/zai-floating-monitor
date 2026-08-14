use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// 单个模型的三项单价（USD/百万 token）。
/// 注：input_tokens 已包含 cache_read_tokens，计费时缓存读部分单独按缓存价计算，
/// 因此非缓存输入 = input_tokens - cache_read_tokens。
/// 人民币不再单独存价：展示/计费时按「美元价 × 当前汇率」实时折算。
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModelPrice {
    pub input: f64,
    pub output: f64,
    pub cache_read: f64,
}

impl Default for ModelPrice {
    fn default() -> Self {
        Self {
            input: 0.0,
            output: 0.0,
            cache_read: 0.0,
        }
    }
}

/// 完整价格配置：只存美元价（人民币按汇率自动折算，不再手工维护两套价格）。
/// 兼容旧版 pricing.json：其中已废弃的 cny 字段会被 serde 忽略，不影响解析。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// key = "model_id"，便于前端按模型查找
    pub usd: BTreeMap<String, ModelPrice>,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            usd: BTreeMap::new(),
        }
    }
}

/// ~/.zbar/ 目录
pub fn config_dir() -> Result<PathBuf, String> {
    let home = dirs::home_dir().ok_or("无法定位用户主目录")?;
    Ok(home.join(".zbar"))
}

pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("pricing.json"))
}

/// 读取价格配置；文件不存在则返回默认空配置（不报错）。
pub fn load_pricing() -> Result<PricingConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(PricingConfig::default());
    }
    let data = fs::read_to_string(&path)
        .map_err(|e| format!("读取价格配置失败: {e}"))?;
    serde_json::from_str::<PricingConfig>(&data)
        .map_err(|e| format!("解析价格配置失败: {e}"))
}

/// 写入价格配置。
pub fn save_pricing(cfg: &PricingConfig) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化价格配置失败: {e}"))?;
    fs::write(&path, data).map_err(|e| format!("写入价格配置失败: {e}"))
}

// ===== 内置参考价格表 + 差异检查（用于"检查更新"提示，绝不自动覆盖）=====

/// 编译期嵌入的内置参考价格表（public/pricing-defaults.json，USD/百万 token）。
/// 定价数据源自 cc-switch 开源项目的成本定价模块，另有新模型时在本文件补充发布。
const DEFAULTS_JSON: &str = include_str!("../../public/pricing-defaults.json");

/// 内置默认表的反序列化结构（多一个 version / note 字段）。
#[derive(Debug, Deserialize)]
struct PricingDefaults {
    #[serde(default)]
    version: String,
    #[serde(default)]
    usd: BTreeMap<String, ModelPrice>,
}

/// 读取内置默认表（解析失败时返回空表，保证不阻塞主流程）。
fn load_defaults() -> PricingDefaults {
    serde_json::from_str::<PricingDefaults>(DEFAULTS_JSON).unwrap_or_else(|_| PricingDefaults {
        version: String::new(),
        usd: BTreeMap::new(),
    })
}

/// 单条差异（模型级）：判定基准 = 内置表 USD 原始价。
/// new_models = 参考有、用户未配置；changed = 用户已配但三项不等。
#[derive(Debug, Clone, Serialize)]
pub struct PriceDiffItem {
    /// 模型 id
    pub model_id: String,
    /// 用户当前 USD 价格（新增模型时为 None）
    pub user: Option<ModelPrice>,
    /// 参考 USD 价格（每百万 token）
    pub default: ModelPrice,
    /// 变体名回退匹配时实际命中的参考表模型 id（如 "gpt-5.6-sol" 命中 "gpt-5"
    /// 的参考价），供前端标注"参考自 xxx"；精确/点号归一命中时为 None
    pub reference_id: Option<String>,
}

/// 完整差异结果
#[derive(Debug, Clone, Serialize)]
pub struct PricingDiff {
    /// 内置参考表版本号
    pub version: String,
    /// 新增模型（参考有、用户未配置），默认勾选应用
    pub new_models: Vec<PriceDiffItem>,
    /// USD 价格变动（用户已配但与参考不同），默认不勾选以保护用户自定义
    pub changed: Vec<PriceDiffItem>,
    /// 实际在用但「参考表与本地配置都没有价格」的模型（花费按 0 计，需手动补价）
    pub missing: Vec<String>,
}

/// 对比用户当前 pricing 与内置参考表，返回差异（纯本地对比，无网络请求）。
/// 判定"是否变动"只看 USD 原始价（价格都是显式写死的配置值，用 == 比较即可）。
///
/// `relevant`: 实际调用过 ∪ 用户已配置的模型 id —— **遍历主体**，
/// 保证实际在用但参考表没收录的模型也能以 missing 暴露出来。
pub fn diff_pricing(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
) -> PricingDiff {
    let d = load_defaults();
    diff_with_reference(user, relevant, &d.usd, &d.version)
}

/// 参考价三级查找（解决 CLI 模型名与参考表收录名不一致的问题）：
/// - L1 精确 / 点号归一："claude-sonnet-4-5"（CLI 落盘名）与参考表的
///   "claude-sonnet-4.5" 是同一模型的两种写法，'.' 与 '-' 统一后比较，
///   视为精确命中、不标注来源；
/// - L2 渐进去尾回退："gpt-5.6-sol" → "gpt-5.6" → "gpt-5"，命中即停，
///   视为变体名（-sol/-terra/-air/-max/日期后缀等）匹配基础模型参考价，
///   返回命中 id 供条目标注"参考自 xxx"，由用户知情勾选；
/// - 均未命中 → missing（未配价警示）。
/// 点号/连字符归一（"claude-sonnet-4.5" ↔ "claude-sonnet-4-5" 视为同一写法）。
/// pub(crate)：lib.rs 的 cost_for 计费查找同样需要归一兜底。
pub(crate) fn normalize_dots(s: &str) -> String {
    s.replace('.', "-")
}


/// 点号归一索引：归一 key → 参考表原始 key（参考表 key 全小写）
fn build_norm_index(map: &BTreeMap<String, ModelPrice>) -> BTreeMap<String, String> {
    map.iter()
        .filter(|(k, _)| k.contains('.'))
        .map(|(k, _)| (normalize_dots(k), k.clone()))
        .collect()
}

/// L1 查找：精确 → 点号归一。命中视为"参考表收录了该模型"。
fn lookup_exact<'a>(
    map: &'a BTreeMap<String, ModelPrice>,
    norm: &BTreeMap<String, String>,
    lc: &str,
) -> Option<&'a ModelPrice> {
    if let Some(p) = map.get(lc) {
        return Some(p);
    }
    norm.get(&normalize_dots(lc)).and_then(|orig| map.get(orig))
}

/// L2 查找：渐进去尾回退（'-' 与 '.' 都算分段边界，如 "gpt-5.6-sol" → "gpt-5.6"
/// → "gpt-5"），每级同时尝试原始与点号归一形态，命中即停。
/// 返回 (参考价, 命中的参考表模型 id)。
fn lookup_variant<'a>(
    map: &'a BTreeMap<String, ModelPrice>,
    norm: &BTreeMap<String, String>,
    lc: &str,
) -> Option<(&'a ModelPrice, String)> {
    let mut cur = lc.to_string();
    loop {
        // 取最靠右的 '-' 或 '.' 作为切点，保证每段（含点号段）都能被剥掉
        let cut = cur.rfind('-').max(cur.rfind('.'));
        let Some(pos) = cut else { break };
        cur.truncate(pos);
        if let Some(p) = map.get(&cur) {
            return Some((p, cur));
        }
        if let Some(orig) = norm.get(&normalize_dots(&cur)) {
            if let Some(p) = map.get(orig) {
                return Some((p, orig.clone()));
            }
        }
    }
    None
}

/// 纯对比逻辑（便于测试）：参考价 map 的 key 需为小写模型 id。
/// 判定基准 = 参考表 USD 原始价。
fn diff_with_reference(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
    ref_usd: &BTreeMap<String, ModelPrice>,
    version: &str,
) -> PricingDiff {
    // 小写归一：db 里的 model_id 大小写可能与参考表不一致（参考表全小写）。
    // 用户配置按「小写 + 点号归一」建索引：db 原始 id 能命中用户以另一种大小写
    // 或点号/连字符形态保存的价格（手输 claude-sonnet-4.5 ↔ db 的 claude-sonnet-4-5）
    let user_usd: BTreeMap<String, &ModelPrice> = user
        .usd
        .iter()
        .map(|(k, v)| (normalize_dots(&k.to_lowercase()), v))
        .collect();
    let norm_usd = build_norm_index(ref_usd);

    let mut new_models = Vec::new();
    let mut changed = Vec::new();
    let mut missing = Vec::new();
    let mut relevant_sorted: Vec<&String> = relevant.iter().collect();
    relevant_sorted.sort();
    // relevant 可能同时含同一模型的多种形态（db 原始 id + 用户配置 key，
    // 大小写或点号/连字符写法不同），归一去重避免同一模型输出多条条目
    let mut seen = std::collections::HashSet::new();

    for model_id in relevant_sorted {
        let lc = model_id.to_lowercase();
        let key = normalize_dots(&lc);
        if !seen.insert(key.clone()) {
            continue;
        }
        let user_u: Option<ModelPrice> = user_usd.get(&key).map(|p| (*p).clone());

        // L1（精确/点号归一）命中：参考表收录了该模型，正常走 new/changed 判定
        if let Some(default_usd) = lookup_exact(ref_usd, &norm_usd, &lc) {
            let item = |user: Option<ModelPrice>| PriceDiffItem {
                model_id: model_id.clone(),
                user,
                default: default_usd.clone(),
                reference_id: None,
            };

            match user_u {
                // 已配 USD：三项不等才提示变动（默认不勾，保护用户自定义）
                Some(u) => {
                    let same = u.input == default_usd.input
                        && u.output == default_usd.output
                        && u.cache_read == default_usd.cache_read;
                    if !same {
                        changed.push(item(Some(u)));
                    }
                }
                // 未配置 → 新增模型（默认勾选，一键应用参考价）
                None => {
                    new_models.push(item(None));
                }
            }
            continue;
        }

        // L1 未命中但本地已配 → 用户自定义价格，不用近似参考去打扰
        if user_u.is_some() {
            continue;
        }
        // L2 变体回退：未配置模型拿基础模型参考价兜底（如 gpt-5.6-sol → gpt-5），
        // 条目标注实际参考来源，默认勾选、由用户知情决定是否应用
        if let Some((usd_ref, hit_id)) = lookup_variant(ref_usd, &norm_usd, &lc) {
            new_models.push(PriceDiffItem {
                model_id: model_id.clone(),
                user: None,
                default: usd_ref.clone(),
                reference_id: Some(hit_id),
            });
            continue;
        }
        // 三级均未命中 → missing（花费按 0，最该提醒手动补价）
        missing.push(model_id.clone());
    }

    PricingDiff {
        version: version.to_string(),
        new_models,
        changed,
        missing,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn price(input: f64, output: f64, cache_read: f64) -> ModelPrice {
        ModelPrice {
            input,
            output,
            cache_read,
        }
    }

    /// diff 分类：new / changed / missing + 大小写归一
    #[test]
    fn diff_classifies_new_changed_missing() {
        let mut user = PricingConfig::default();
        // glm-4.5：与参考一致 → 无条目
        user.usd.insert("glm-4.5".into(), price(0.7, 2.1, 0.07));
        // glm-4-air：与参考不同 → changed
        user.usd.insert("glm-4-air".into(), price(0.1, 0.4, 0.01));
        // gpt-5：用户小写配置 + relevant 混入 db 大写形态，归一后一致 → 无条目
        user.usd.insert("gpt-5".into(), price(1.25, 10.0, 0.125));

        let relevant: std::collections::HashSet<String> = [
            "glm-4.5".to_string(),
            "GLM-4.6".to_string(), // 数据库大写，未配置 → 新增（保留 db 原始 id）
            "glm-4-air".to_string(),
            "GPT-5".to_string(), // 与用户小写 key 归一去重
            "gpt-5".to_string(),
            "glm-x-new".to_string(), // 实际在用但参考表与本地都无 → missing
        ]
        .into_iter()
        .collect();

        let ref_usd = BTreeMap::from([
            ("glm-4.5".to_string(), price(0.7, 2.1, 0.07)),
            ("glm-4.6".to_string(), price(0.6, 2.2, 0.11)),
            ("glm-4-air".to_string(), price(0.11, 0.42, 0.011)),
            ("gpt-5".to_string(), price(1.25, 10.0, 0.125)),
        ]);

        let diff = diff_with_reference(&user, &relevant, &ref_usd, "test");

        // 新增：仅 glm-4.6（gpt-5 归一后已配置）
        assert_eq!(diff.new_models.len(), 1);
        assert_eq!(diff.new_models[0].model_id, "GLM-4.6");
        assert_eq!(diff.new_models[0].default, price(0.6, 2.2, 0.11));
        // 变动：仅 glm-4-air（三项不等）
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].model_id, "glm-4-air");
        assert_eq!(diff.changed[0].user, Some(price(0.1, 0.4, 0.01)));
        // missing：glm-x-new
        assert_eq!(diff.missing, vec!["glm-x-new".to_string()]);
        assert_eq!(diff.version, "test");
    }

    /// 变体名三级匹配：点号归一视为精确、去尾回退标注参考来源、已配置不近似打扰
    #[test]
    fn diff_matches_variant_model_names() {
        let user = PricingConfig::default();
        // 场景数据：参考表只有基础模型（模拟 gpt-5.6 系列尚未收录）
        let ref_usd = BTreeMap::from([
            ("gpt-5".to_string(), price(1.25, 10.0, 0.125)),
            ("gpt-5.5".to_string(), price(1.5, 12.0, 0.15)),
            ("claude-sonnet-4.5".to_string(), price(3.0, 15.0, 0.3)),
            ("glm-4.5".to_string(), price(0.5, 2.0, 0.05)),
        ]);

        let relevant: std::collections::HashSet<String> = [
            "gpt-5.6-sol".to_string(),          // 两级去尾 → gpt-5，标注参考来源
            "gpt-5.6-terra".to_string(),        // 同上
            "gpt-5.5-codex-low".to_string(),    // 一级去尾 → gpt-5.5（含点号，直接命中）
            "claude-sonnet-4-5".to_string(),    // 点号归一 → claude-sonnet-4.5，视为精确
            "glm-4.5-air".to_string(),          // 一级去尾 → glm-4.5
            "glm-x-unknown".to_string(),        // 三级均 miss → missing
        ]
        .into_iter()
        .collect();

        let diff = diff_with_reference(&user, &relevant, &ref_usd, "test");

        assert_eq!(diff.new_models.len(), 5, "五个未配置模型都应有参考价");
        let by_id: std::collections::HashMap<&str, &PriceDiffItem> = diff
            .new_models
            .iter()
            .map(|i| (i.model_id.as_str(), i))
            .collect();
        // 点号归一命中视为精确，不标注来源
        assert_eq!(by_id["claude-sonnet-4-5"].reference_id, None);
        assert_eq!(by_id["claude-sonnet-4-5"].default, price(3.0, 15.0, 0.3));
        // 去尾回退命中标注实际参考来源
        assert_eq!(by_id["gpt-5.6-sol"].reference_id, Some("gpt-5".to_string()));
        assert_eq!(by_id["gpt-5.6-terra"].reference_id, Some("gpt-5".to_string()));
        assert_eq!(
            by_id["gpt-5.5-codex-low"].reference_id,
            Some("gpt-5.5".to_string())
        );
        assert_eq!(by_id["glm-4.5-air"].reference_id, Some("glm-4.5".to_string()));
        assert_eq!(by_id["gpt-5.6-sol"].default, price(1.25, 10.0, 0.125));
        // 三级均 miss → missing
        assert_eq!(diff.missing, vec!["glm-x-unknown".to_string()]);
    }

    /// 已配置的变体模型不被近似参考打扰：用户给 gpt-5.6-sol 手动配过价时，
    /// 不拿 gpt-5 的参考价去提示变动（近似参考只服务未配置模型的首次配价）
    #[test]
    fn diff_skips_configured_variant_models() {
        let mut user = PricingConfig::default();
        user.usd.insert(
            "gpt-5.6-sol".to_string(),
            price(1.25, 10.0, 0.125),
        );
        let ref_usd = BTreeMap::from([("gpt-5".to_string(), price(2.0, 20.0, 0.2))]);

        let relevant: std::collections::HashSet<String> =
            ["gpt-5.6-sol".to_string()].into_iter().collect();
        let diff = diff_with_reference(&user, &relevant, &ref_usd, "test");

        assert!(diff.new_models.is_empty());
        assert!(diff.changed.is_empty(), "不拿基础模型参考价判变动");
        assert!(diff.missing.is_empty(), "已配置不算 missing");
    }

    /// 用户手输点号形态（claude-sonnet-4.5）与 db 连字符形态（claude-sonnet-4-5）
    /// 视为同一模型：已配置不误报新增，应用后收敛为单一 key
    #[test]
    fn diff_normalizes_user_dot_notation() {
        let mut user = PricingConfig::default();
        // 用户照抄参考表形态手动配置的价格
        user.usd.insert("claude-sonnet-4.5".to_string(), price(3.0, 15.0, 0.3));

        let relevant: std::collections::HashSet<String> = [
            "claude-sonnet-4-5".to_string(), // db 落盘形态
            "claude-sonnet-4.5".to_string(),  // 用户配置 key（归一后与上一条同模型）
        ]
        .into_iter()
        .collect();
        let ref_usd = BTreeMap::from([("claude-sonnet-4.5".to_string(), price(3.0, 15.0, 0.3))]);

        let diff = diff_with_reference(&user, &relevant, &ref_usd, "test");
        // 已配置（点号形态命中）且与参考一致 → 不产出任何条目
        assert!(diff.new_models.is_empty(), "用户点号配置应被识别，不误报新增");
        assert!(diff.changed.is_empty());
        assert!(diff.missing.is_empty());

        // 应用写入连字符形态时收敛掉点号旧 key
        let mut map = BTreeMap::from([("claude-sonnet-4.5".to_string(), price(1.0, 5.0, 0.1))]);
        collapse_insert(&mut map, "claude-sonnet-4-5", price(3.0, 15.0, 0.3));
        assert_eq!(map.len(), 1, "点号/连字符形态应收敛为单一 key");
        assert!(map.contains_key("claude-sonnet-4-5"));
    }

    /// apply_updates 写入前收敛同模型大小写重复 key：
    /// 否则归一索引取旧值、应用写入新形态，changed 条目应用后仍不清零
    #[test]
    fn collapse_insert_removes_case_variants() {
        let mut map = BTreeMap::from([
            ("glm-4.6".to_string(), price(0.6, 2.2, 0.11)), // 手输小写旧值
            ("gpt-5".to_string(), price(1.25, 10.0, 0.125)), // 无关模型，应保留
        ]);
        collapse_insert(&mut map, "GLM-4.6", price(0.35, 1.4, 0.035));
        assert_eq!(map.len(), 2, "同模型应收敛为单一 key");
        assert_eq!(map.get("GLM-4.6"), Some(&price(0.35, 1.4, 0.035)));
        assert!(map.get("glm-4.6").is_none(), "旧小写形态应被清除");
        assert!(map.contains_key("gpt-5"), "其他模型不受影响");
    }

    /// 内置参考表可解析且条目非空（防止 JSON 格式损坏后静默退化为空表）
    #[test]
    fn builtin_defaults_load_nonempty() {
        let d = load_defaults();
        assert!(!d.version.is_empty(), "内置表应带版本号");
        assert!(d.usd.len() > 100, "内置表条目异常少: {}", d.usd.len());
        // 关键模型抽查（数据源自 cc-switch 成本定价模块）
        assert_eq!(d.usd["gpt-5.6-sol"], price(5.0, 30.0, 0.5));
        assert_eq!(d.usd["glm-4.6"], price(0.6, 2.2, 0.11));
        assert_eq!(d.usd["claude-sonnet-5"], price(3.0, 15.0, 0.3));
        assert!(d.usd.contains_key("glm-4.5"), "Z.ai 特有模型应保留");
    }
}

/// 把用户勾选的若干 (model_id, currency, price) 合并进 pricing 并保存。
/// 已存在的会被覆盖（用户主动勾选即视为同意）。
/// currency 参数保留以兼容前端结构，实际一律写入 usd（只存美元价）。
pub fn apply_updates(items: &[(String, String, ModelPrice)]) -> Result<PricingConfig, String> {
    let mut cfg = load_pricing()?;
    for (model_id, _currency, price) in items {
        collapse_insert(&mut cfg.usd, model_id, price.clone());
    }
    save_pricing(&cfg)?;
    Ok(cfg)
}

/// 删除同模型其他写法（大小写、点号/连字符）的旧 key 后写入（收敛为单一 key）。
/// 如手输的 "glm-4.6" / "claude-sonnet-4.5" 与应用写入的 "GLM-4.6" /
/// "claude-sonnet-4-5" 并存时，diff 的归一索引会取到旧值，
/// 导致应用后条目仍不清零、红点反复出现。
fn collapse_insert(map: &mut BTreeMap<String, ModelPrice>, model_id: &str, price: ModelPrice) {
    let same_model = |k: &str, target: &str| {
        normalize_dots(&k.to_lowercase()) == normalize_dots(&target.to_lowercase())
    };
    map.retain(|k, _| !same_model(k, model_id));
    map.insert(model_id.to_string(), price);
}

// ===== 货币偏好（菜单栏标题据此显示 ¥ / $）=====

/// 读取货币偏好；文件不存在或非法时返回 "cny"。
pub fn load_currency() -> String {
    let path = match config_dir() {
        Ok(d) => d.join("currency.json"),
        Err(_) => return "cny".to_string(),
    };
    fs::read_to_string(&path)
        .ok()
        .and_then(|s| serde_json::from_str::<CurrencyPref>(&s).ok())
        .map(|c| if c.currency == "usd" { "usd" } else { "cny" }.to_string())
        .unwrap_or_else(|| "cny".to_string())
}

/// 保存货币偏好。
pub fn save_currency(currency: &str) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = dir.join("currency.json");
    let pref = CurrencyPref {
        currency: currency.to_string(),
    };
    let data = serde_json::to_string_pretty(&pref)
        .map_err(|e| format!("序列化货币偏好失败: {e}"))?;
    fs::write(&path, data).map_err(|e| format!("写入货币偏好失败: {e}"))
}

#[derive(Debug, Serialize, Deserialize)]
struct CurrencyPref {
    currency: String,
}

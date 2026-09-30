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

    /// models.dev 解析（白名单 + 真实接口形态的小样例）：
    /// 白名单外厂商跳过、key 取 "/" 最后一段并小写、cost 缺失字段按 0
    #[test]
    fn modelsdev_parse_filters_whitelist_and_normalizes_keys() {
        // 样例结构照 /tmp/modelsdev.json 实测：顶层按 provider 分组，
        // 模型带 cost{input,output,cache_read,cache_write}（cache_write 不入库）
        let sample = r#"{
            "deepinfra": { "models": {
                "meta-llama/Llama-3.3-70B-Instruct-Turbo": {
                    "cost": { "input": 0.01, "output": 0.02, "cache_read": 0.001 }
                }
            }},
            "zai": { "models": {
                "glm-4.6": { "cost": { "input": 0.6, "output": 2.2, "cache_read": 0.11, "cache_write": 0.0 } },
                "GLM-4.5-Air": { "cost": { "input": 0.11, "output": 0.42 } }
            }},
            "anthropic": { "models": {
                "claude-sonnet-4-5": { "cost": { "input": 3.0, "output": 15.0, "cache_read": 0.3 } },
                "org-prefix/claude-haiku-4-5": { "cost": { "input": 1.0, "output": 5.0, "cache_read": 0.1 } }
            }},
            "openai": { "models": {
                "gpt-image-1": { "id": "gpt-image-1" },
                "gpt-image-2": { "cost": null }
            }},
            "moonshotai-cn": { "models": {
                "kimi-k2.7-code": { "cost": { "input": 1.0, "output": 5.0, "cache_read": 0.1 } }
            }},
            "qwen": {}
        }"#;
        let root: serde_json::Value = serde_json::from_str(sample).unwrap();
        let m = parse_modelsdev_prices(&root);

        // 白名单外（deepinfra）不收录
        assert!(!m.contains_key("llama-3.3-70b-instruct-turbo"), "白名单外厂商应跳过");
        // 白名单内但无 models 的厂商（qwen）自然跳过，不影响其他厂商
        assert_eq!(m.get("glm-4.6"), Some(&price(0.6, 2.2, 0.11)));
        // 模型 id 大写归一小写；cost 缺 cache_read 按 0
        assert_eq!(m.get("glm-4.5-air"), Some(&price(0.11, 0.42, 0.0)));
        // 带 org 前缀的 id 取 "/" 最后一段
        assert_eq!(m.get("claude-haiku-4-5"), Some(&price(1.0, 5.0, 0.1)));
        assert_eq!(m.get("claude-sonnet-4-5"), Some(&price(3.0, 15.0, 0.3)));
        // Moonshot 官方价实际挂在 moonshotai / moonshotai-cn（moonshot id 不存在）
        assert_eq!(m.get("kimi-k2.7-code"), Some(&price(1.0, 5.0, 0.1)));
        // 完全没有 cost 的条目（官方未公布价格）跳过，不生成全 0 参考价
        assert!(!m.contains_key("gpt-image-1"), "无 cost 的条目应跳过");
        // cost 为 JSON null 同样视为未公布，跳过不生成全 0 参考价
        assert!(!m.contains_key("gpt-image-2"), "cost=null 的条目应跳过");
    }

    /// 同一 key 重复（多家厂商各有一份同 id 价）保留首个，且
    /// 提取结果可直接喂给 diff_with_reference 当参考表
    #[test]
    fn modelsdev_duplicate_key_keeps_first_and_feeds_diff() {
        let sample = r#"{
            "openai": { "models": {
                "gpt-5": { "cost": { "input": 1.25, "output": 10.0, "cache_read": 0.125 } }
            }},
            "azure": { "models": {
                "gpt-5": { "cost": { "input": 9.9, "output": 9.9, "cache_read": 9.9 } }
            }}
        }"#;
        let root: serde_json::Value = serde_json::from_str(sample).unwrap();
        let ref_usd = parse_modelsdev_prices(&root);
        assert_eq!(
            ref_usd.get("gpt-5"),
            Some(&price(1.25, 10.0, 0.125)),
            "重复 key 应保留首个（openai）"
        );

        // 提取表可作为 diff_with_reference 的参考表（在线同步的复用路径）：
        // 未配置 → new，参考表与本地都没有 → missing。
        // 注意第二个 id 不能形如 "gpt-5-xxx"——变体名回退会命中 gpt-5 进 new
        let user = PricingConfig::default();
        let relevant: std::collections::HashSet<String> =
            ["gpt-5".to_string(), "totally-unknown".to_string()]
                .into_iter()
                .collect();
        let diff = diff_with_reference(&user, &relevant, &ref_usd, MODELSDEV_VERSION);
        assert_eq!(diff.new_models.len(), 1);
        assert_eq!(diff.new_models[0].model_id, "gpt-5");
        assert_eq!(diff.new_models[0].default, price(1.25, 10.0, 0.125));
        // 参考表没有的模型照常走 missing
        assert_eq!(diff.missing, vec!["totally-unknown".to_string()]);
        assert_eq!(diff.version, "models.dev");

        let mut configured = PricingConfig::default();
        configured.usd.insert("gpt-5".to_string(), price(2.0, 8.0, 0.1));
        let diff = diff_with_reference(&configured, &relevant, &ref_usd, MODELSDEV_VERSION);
        assert_eq!(diff.changed.len(), 1, "与官方价不同应报 changed");
        assert_eq!(diff.changed[0].default, price(1.25, 10.0, 0.125));
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

// ===== models.dev 官方价格在线同步（用户手动触发，含本地缓存降级）=====

/// models.dev 在线价格源（GET 无鉴权，约 5MB JSON；顶层按 provider 分组）
const MODELSDEV_URL: &str = "https://models.dev/api.json";
/// 在线参考表的版本号（前端差异面板标题直接展示该值）
pub const MODELSDEV_VERSION: &str = "models.dev";

/// 只采信这些官方厂商的自营价格：社区/中转站在 models.dev 上的价格不可信，
/// 白名单外的厂商直接跳过（不存在的 id 同样自然跳过，不报错）。
const MODELSDEV_PROVIDERS: &[&str] = &[
    "anthropic",
    "openai",
    "google",
    "zai",
    "deepseek",
    // Moonshot 官方价实际挂在 moonshotai / moonshotai-cn 两家（models.dev 上
    // 不存在 moonshot、baidu 这两个 id）：两家模型集互补，同 key 保留首条的
    // 解析语义下互补模型都能入库
    "moonshotai",
    "moonshotai-cn",
    "minimax",
    "xai",
    "mistral",
    "cohere",
    "alibaba",
    "qwen",
    "stepfun",
    "perplexity",
    "amazon-bedrock",
    "azure",
];

/// models.dev 同步结果的本地缓存（~/.zbar/modelsdev-cache.json）。
/// 只存白名单厂商提取后的参考表（数百条，远小于源 JSON）：
/// 网络失败时降级用上次成功数据，保证离线也能出差异。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsdevCache {
    /// 抓取时间（ms 时间戳，前端展示"数据时间"）
    pub fetched_at: i64,
    /// 官方参考价（USD/百万 token），key = 小写模型 id
    pub usd: BTreeMap<String, ModelPrice>,
}

/// 缓存文件路径
fn modelsdev_cache_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("modelsdev-cache.json"))
}

/// 读取 modelsdev 缓存；文件不存在或损坏返回 Err（调用方决定是否报错）
fn load_modelsdev_cache() -> Result<ModelsdevCache, String> {
    let path = modelsdev_cache_path()?;
    if !path.exists() {
        return Err("暂无 models.dev 本地缓存".to_string());
    }
    let data =
        fs::read_to_string(&path).map_err(|e| format!("读取 models.dev 缓存失败: {e}"))?;
    serde_json::from_str(&data).map_err(|e| format!("解析 models.dev 缓存失败: {e}"))
}

/// 写入 modelsdev 缓存
fn save_modelsdev_cache(cache: &ModelsdevCache) -> Result<(), String> {
    let dir = config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let data =
        serde_json::to_string(cache).map_err(|e| format!("序列化 models.dev 缓存失败: {e}"))?;
    fs::write(modelsdev_cache_path()?, data).map_err(|e| format!("写入 models.dev 缓存失败: {e}"))
}

/// 从 models.dev 顶层 JSON（serde_json::Value，避免为 225 家厂商建全量结构体）
/// 提取白名单厂商的参考价表。纯函数，供在线同步与单测复用：
/// - key = model_id 的 "/" 最后一段 to_lowercase（官方家多为裸 id，个别带
///   org 前缀；与本应用"参考表 key 全小写"的口径对齐）；
/// - 同一 key 重复时保留首个（不同厂商偶然同名时以先遍历到的为准，不抖动）；
/// - cost 缺失或为 null 的条目（官方未公布价格）跳过；cost 存在但缺
///   input/output/cache_read 某字段时按 0.0；cache_write 不入库
///   （本应用计费口径只有三项）。
fn parse_modelsdev_prices(root: &serde_json::Value) -> BTreeMap<String, ModelPrice> {
    let mut out = BTreeMap::new();
    for pid in MODELSDEV_PROVIDERS {
        let Some(models) = root
            .get(*pid)
            .and_then(|p| p.get("models"))
            .and_then(|m| m.as_object())
        else {
            continue;
        };
        for (model_id, mv) in models {
            // cost 需存在且为对象才入表：完全没有 cost（如 openai 的图像模型）或
            // cost 为 JSON null 都是官方未公布价格，跳过而不是按全 0 入表，否则会
            // 把用户已配的价格误报成"变动 → $0"（new_models 默认勾选，一键应用即写 0）。
            // cost 存在但缺 input/output/cache_read 某个字段时才按 0.0
            let Some(cost_obj) = mv.get("cost").and_then(|c| c.as_object()) else {
                continue;
            };
            let key = model_id.rsplit('/').next().unwrap_or(model_id).to_lowercase();
            let cost = |name: &str| {
                cost_obj
                    .get(name)
                    .and_then(|v| v.as_f64())
                    .unwrap_or(0.0)
            };
            out.entry(key).or_insert(ModelPrice {
                input: cost("input"),
                output: cost("output"),
                cache_read: cost("cache_read"),
            });
        }
    }
    out
}

/// 从 models.dev 在线同步官方厂商价格并生成差异（复用内置检查的判定逻辑）。
/// 返回 (差异, 数据抓取时间 ms, 是否来自缓存)。
/// - 网络走 net_config 的统一代理配置（空 = 直连），读取超时 30s；
/// - 成功后写缓存（fetched_at = 当前毫秒）；写入失败仅记日志不阻断（缓存
///   只是降级手段，不影响本次结果）；
/// - 拉取/解析失败时降级用本地合法缓存（from_cache = true）；无缓存才报错。
pub fn fetch_modelsdev_diff() -> Result<(PricingDiff, i64, bool), String> {
    let fetched: Result<(BTreeMap<String, ModelPrice>, i64), String> = (|| {
        let agent = crate::net_config::http_agent(30)?;
        let resp = agent
            .get(MODELSDEV_URL)
            .set("Accept", "application/json")
            .call()
            .map_err(|e| format!("拉取 models.dev 价格失败: {e}"))?;
        let root: serde_json::Value = resp
            .into_json()
            .map_err(|e| format!("解析 models.dev 响应失败: {e}"))?;
        let prices = parse_modelsdev_prices(&root);
        // 空表说明响应结构与预期不符（如被网关劫持返回错误页 JSON）：
        // 不能当"白名单厂商都没价"处理，否则会误导用户清空参考价
        if prices.is_empty() {
            return Err("models.dev 响应中未找到白名单厂商的价格数据".to_string());
        }
        Ok((prices, chrono::Utc::now().timestamp_millis()))
    })();

    let (ref_usd, fetched_at, from_cache) = match fetched {
        Ok((prices, at)) => {
            let cache = ModelsdevCache {
                fetched_at: at,
                usd: prices.clone(),
            };
            if let Err(e) = save_modelsdev_cache(&cache) {
                eprintln!("[zbar-pricing] models.dev 缓存写入失败: {e}");
            }
            (prices, at, false)
        }
        Err(net_err) => match load_modelsdev_cache() {
            // 合法缓存才降级：空表缓存与无缓存同等对待
            Ok(cache) if !cache.usd.is_empty() => {
                eprintln!("[zbar-pricing] models.dev 拉取失败，降级用本地缓存: {net_err}");
                (cache.usd, cache.fetched_at, true)
            }
            _ => return Err(net_err),
        },
    };

    // 遍历主体与判定逻辑与内置检查完全一致（collect_relevant_models 在 lib.rs，
    // 两种来源的差异口径共用一套，避免同一模型两处结论矛盾）
    let user = load_pricing()?;
    let relevant = crate::collect_relevant_models(&user)?;
    let diff = diff_with_reference(&user, &relevant, &ref_usd, MODELSDEV_VERSION);
    Ok((diff, fetched_at, from_cache))
}

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// 单个模型的三项单价（每百万 token）。各货币各存一份。
/// 注：input_tokens 已包含 cache_read_tokens，计费时缓存读部分单独按缓存价计算，
/// 因此非缓存输入 = input_tokens - cache_read_tokens。
#[derive(Debug, Clone, Serialize, Deserialize)]
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

/// 完整价格配置：两套货币
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PricingConfig {
    /// key = "model_id"，便于前端按模型查找
    pub cny: BTreeMap<String, ModelPrice>,
    pub usd: BTreeMap<String, ModelPrice>,
}

impl Default for PricingConfig {
    fn default() -> Self {
        Self {
            cny: BTreeMap::new(),
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

// ===== 内置默认价格表 + 差异检查（用于"检查更新"提示，绝不自动覆盖）=====

/// 编译期嵌入的内置参考价格表（public/pricing-defaults.json）。
/// 维护者随官方调价更新此文件并重新发布即可，无需网络。
const DEFAULTS_JSON: &str = include_str!("../../public/pricing-defaults.json");

/// 内置默认表的反序列化结构（多一个 version / note 字段）。
#[derive(Debug, Deserialize)]
struct PricingDefaults {
    #[serde(default)]
    version: String,
    #[serde(default)]
    cny: BTreeMap<String, ModelPrice>,
    #[serde(default)]
    usd: BTreeMap<String, ModelPrice>,
}

/// 读取内置默认表（解析失败时返回空表，保证不阻塞主流程）。
fn load_defaults() -> PricingDefaults {
    serde_json::from_str::<PricingDefaults>(DEFAULTS_JSON).unwrap_or_else(|_| PricingDefaults {
        version: String::new(),
        cny: BTreeMap::new(),
        usd: BTreeMap::new(),
    })
}

/// 单条差异：用户本地与默认价格不一致的某个货币维度。
/// new_models = 默认有、用户本地没有；changed = 两边都有但三项价格不完全相同。
#[derive(Debug, Clone, Serialize)]
pub struct PriceDiffItem {
    /// 模型 id
    pub model_id: String,
    /// "cny" | "usd"
    pub currency: String,
    /// 用户当前价格（new 模型时为 None）
    pub user: Option<ModelPrice>,
    /// 内置默认价格
    pub default: ModelPrice,
}

/// 完整差异结果
#[derive(Debug, Clone, Serialize)]
pub struct PricingDiff {
    /// 内置表的版本号
    pub version: String,
    /// 新增模型（默认有、用户无），默认勾选应用
    pub new_models: Vec<PriceDiffItem>,
    /// 价格变动（两边都有但不同），默认不勾选以保护用户自定义
    pub changed: Vec<PriceDiffItem>,
}

/// 对比用户当前 pricing 与内置默认表，返回差异。
/// 判定"价格不同"时三项全等才算相同（避免浮点抖动，用 == 比较配置值即可，
/// 因为价格都是用户/默认显式写死的小数，不是计算结果）。
///
/// `relevant`: 只关心这些模型 id（通常是「数据库里出现过 ∪ 用户已手动配置」）。
/// 内置表里用户从没用过的模型不算差异，避免噪音。
pub fn diff_with_defaults(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
) -> PricingDiff {
    let defaults = load_defaults();
    let mut new_models = Vec::new();
    let mut changed = Vec::new();

    for (cur_name, default_map) in [("cny", &defaults.cny), ("usd", &defaults.usd)] {
        let user_map = if cur_name == "cny" { &user.cny } else { &user.usd };
        for (model_id, default_price) in default_map {
            // 跳过用户不关心的模型（既没在数据库出现过，也没手动配置过）
            if !relevant.contains(model_id) {
                continue;
            }
            match user_map.get(model_id) {
                None => {
                    // 用户本地没有 → 新增
                    new_models.push(PriceDiffItem {
                        model_id: model_id.clone(),
                        currency: cur_name.to_string(),
                        user: None,
                        default: default_price.clone(),
                    });
                }
                Some(user_price) => {
                    // 两边都有，比较三项
                    let same = user_price.input == default_price.input
                        && user_price.output == default_price.output
                        && user_price.cache_read == default_price.cache_read;
                    if !same {
                        changed.push(PriceDiffItem {
                            model_id: model_id.clone(),
                            currency: cur_name.to_string(),
                            user: Some(user_price.clone()),
                            default: default_price.clone(),
                        });
                    }
                }
            }
        }
    }

    PricingDiff {
        version: defaults.version,
        new_models,
        changed,
    }
}

/// 把用户勾选的若干 (model_id, currency, price) 合并进 pricing 并保存。
/// 已存在的会被覆盖（用户主动勾选即视为同意）。
pub fn apply_updates(items: &[(String, String, ModelPrice)]) -> Result<PricingConfig, String> {
    let mut cfg = load_pricing()?;
    for (model_id, currency, price) in items {
        let map = if currency == "cny" {
            &mut cfg.cny
        } else {
            &mut cfg.usd
        };
        map.insert(model_id.clone(), price.clone());
    }
    save_pricing(&cfg)?;
    Ok(cfg)
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

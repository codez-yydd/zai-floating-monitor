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
/// 作为 models.dev 不可达时的离线兜底；主数据源见 fetch_models_dev_prices。
const DEFAULTS_JSON: &str = include_str!("../../public/pricing-defaults.json");

/// models.dev 全厂商模型目录（USD / 百万 token，社区维护，随官方调价更新）
const MODELS_DEV_URL: &str = "https://models.dev/api.json";
/// models.dev 响应缓存有效期（磁盘缓存 ~/.zbar/models-dev-cache.json）
const MODELS_DEV_TTL_MS: u64 = 24 * 3600 * 1000;

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

// ---------- models.dev 拉取 + 磁盘缓存 ----------

/// models.dev 磁盘缓存结构
#[derive(Debug, Serialize, Deserialize)]
struct ModelsDevCache {
    fetched_at: u64,
    /// USD 价格（key 为小写 model id）
    usd: BTreeMap<String, ModelPrice>,
}

fn models_dev_cache_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("models-dev-cache.json"))
}

fn load_models_dev_cache() -> Option<ModelsDevCache> {
    let path = models_dev_cache_path().ok()?;
    let data = fs::read_to_string(path).ok()?;
    serde_json::from_str::<ModelsDevCache>(&data).ok()
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

/// JSON 值 → f64（兼容数值与字符串两种序列化，社区 API 常见保精度用字符串）。
/// 无法解析时返回 None（调用方跳过该字段）。
fn value_as_f64(v: &serde_json::Value) -> Option<f64> {
    match v {
        serde_json::Value::Number(n) => n.as_f64(),
        serde_json::Value::String(s) => s.trim().parse::<f64>().ok(),
        _ => None,
    }
}

/// 官方直连厂商白名单（models.dev provider key，小写比较）。
/// 同一 model_id 会在多个厂商/聚合商下出现（如 glm-4.6 同时在 zhipuai 与 openrouter），
/// 提取时官方价优先，聚合商价仅在官方未收录时兜底。
const OFFICIAL_PROVIDERS: &[&str] = &[
    "zhipuai", "openai", "anthropic", "google", "alibaba", "qwen", "deepseek", "moonshot",
    "moonshotai", "minimax", "01-ai", "yi", "xai", "meta", "mistral", "cohere", "perplexity",
    "amazon", "microsoft", "nvidia", "baidu", "tencent", "stepfun", "ai21", "writer",
];

/// 从 models.dev api.json 提取**全厂商**模型价格（USD/百万 token，key 为小写 model_id）。
/// ZCode 可接入任意厂商模型（qwen / claude / deepseek …），因此不做厂商过滤。
/// 逐模型容错：单个模型字段缺失/类型异常只跳过该模型；同名模型官方厂商价优先。
fn extract_all_prices(root: &serde_json::Value) -> BTreeMap<String, ModelPrice> {
    let Some(providers) = root.as_object() else {
        return BTreeMap::new();
    };

    let mut usd = BTreeMap::new();
    // 两轮遍历：官方厂商先入表（可占位），聚合商后入且不覆盖已有价
    for pass in 0..2 {
        for (provider_key, provider) in providers {
            let is_official = OFFICIAL_PROVIDERS.contains(&provider_key.to_lowercase().as_str());
            if (pass == 0) != is_official {
                continue;
            }
            let Some(models) = provider.get("models").and_then(|m| m.as_object()) else {
                continue;
            };
            for (id, model) in models {
                let Some(cost) = model.get("cost") else { continue };
                // input/output 必须都有且可解析；cache_read 缺省/异常按 0
                let (Some(input), Some(output)) = (
                    cost.get("input").and_then(value_as_f64),
                    cost.get("output").and_then(value_as_f64),
                ) else {
                    continue;
                };
                if input <= 0.0 && output <= 0.0 {
                    continue;
                }
                let key = id.to_lowercase();
                if pass == 1 && usd.contains_key(&key) {
                    continue; // 聚合商价不覆盖已有（官方）价
                }
                usd.insert(
                    key,
                    ModelPrice {
                        input,
                        output,
                        cache_read: cost.get("cache_read").and_then(value_as_f64).unwrap_or(0.0),
                    },
                );
            }
        }
    }
    usd
}

/// models.dev 拉取模式
pub enum FetchMode {
    /// 只读 24h 内缓存；过期/缺失直接回退内置表（不联网，进面板静默检查用）
    CacheOnly,
    /// 缓存未过期用缓存；过期则联网刷新（默认）
    Cached,
    /// 绕过缓存强制联网（手动点「检查更新」按钮）
    Force,
}

/// 拉取 models.dev 全厂商模型 USD 价格（每百万 token）。
/// 磁盘缓存 24h；网络失败时回退过期缓存（宁用昨日数据也不用静态内置表），
/// 再失败返回 Err（调用方用内置表兜底）。
pub fn fetch_models_dev_prices(mode: FetchMode) -> Result<BTreeMap<String, ModelPrice>, String> {
    let cached = load_models_dev_cache();
    if !matches!(mode, FetchMode::Force) {
        if let Some(c) = cached.as_ref() {
            if now_ms().saturating_sub(c.fetched_at) < MODELS_DEV_TTL_MS {
                return Ok(c.usd.clone());
            }
        }
        if matches!(mode, FetchMode::CacheOnly) {
            // 静默检查：不联网，让调用方回退内置表
            return Err("models.dev 缓存不可用（cache-only 模式不联网）".to_string());
        }
    }

    // 多源 failover：官方源国内可能直连不通，依次尝试镜像
    let mut last_err = String::new();
    let mut usd = BTreeMap::new();
    for url in models_dev_urls() {
        match fetch_models_dev_from(&url) {
            Ok(prices) => {
                usd = prices;
                break;
            }
            Err(e) => last_err = e,
        }
    }
    if usd.is_empty() {
        // 全部源失败：回退过期缓存（若有），保持「models.dev 数据源」语义
        if let Some(c) = cached {
            if !c.usd.is_empty() {
                return Ok(c.usd);
            }
        }
        return Err(format!("models.dev 所有数据源均不可达: {last_err}"));
    }

    // 写缓存（失败静默：缓存只是优化）
    let cache = ModelsDevCache {
        fetched_at: now_ms(),
        usd: usd.clone(),
    };
    if let Ok(dir) = config_dir() {
        if fs::create_dir_all(&dir).is_ok() {
            if let Ok(path) = models_dev_cache_path() {
                if let Ok(data) = serde_json::to_string_pretty(&cache) {
                    let _ = fs::write(path, data);
                }
            }
        }
    }
    Ok(usd)
}

/// models.dev 数据源列表（按优先级）：
/// - 官方 api.json（国内可能直连不通）
/// - jsDelivr CDN 镜像 GitHub anomalyco/models.dev 的 models.json（国内通常可达）
/// - fastly 是 jsDelivr 的备用线路（连通性波动时兜底）
///
/// 注：镜像的 models.json 是 canonical 视图（{data:[{id:"z-ai/glm-4.6", pricing:{...}}]}，
/// 价格为 USD/token 字符串），与 api.json 的 provider 视图结构不同，解析时自动分派。
fn models_dev_urls() -> Vec<String> {
    vec![
        MODELS_DEV_URL.to_string(),
        "https://cdn.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json".to_string(),
        "https://fastly.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json".to_string(),
    ]
}

/// 从单个 URL 拉取并提取全厂商价格（超时 8s，按响应结构自动分派视图）
fn fetch_models_dev_from(url: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(8))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("请求失败: {e}"))?;
    let root: serde_json::Value = resp
        .into_json()
        .map_err(|e| format!("响应解析失败: {e}"))?;
    let usd = if root.get("data").and_then(|d| d.as_array()).is_some() {
        extract_from_models_json(&root)
    } else {
        extract_all_prices(&root)
    };
    if usd.is_empty() {
        return Err("未解析到任何模型价格".to_string());
    }
    Ok(usd)
}

/// jsDelivr 镜像 models.json 视图：{data:[{id:"z-ai/glm-4.6", pricing:{prompt, completion,
/// input_cache_read}}]}，价格为 USD/token 字符串 → 换算为 USD/百万 token。
/// 全厂商提取（ZCode 可接入任意厂商模型），剥掉 "厂商/" 前缀；canonical 视图本身已去重。
/// 跳过 ":free" 等变体 id，避免免费端点价覆盖正式价。
fn extract_from_models_json(root: &serde_json::Value) -> BTreeMap<String, ModelPrice> {
    let mut usd = BTreeMap::new();
    let Some(items) = root.get("data").and_then(|d| d.as_array()) else {
        return usd;
    };
    for item in items {
        let Some(id) = item.get("id").and_then(|i| i.as_str()) else { continue };
        // 剥 "厂商/" 前缀（如 "z-ai/glm-4.6" → "glm-4.6"、"alibaba/qwen-max" → "qwen-max"）
        let model_id = id.rsplit('/').next().unwrap_or(id);
        if model_id.is_empty() || model_id.contains(':') {
            continue; // ":free" 等变体端点，非正式定价
        }
        let Some(pricing) = item.get("pricing") else { continue };
        // USD/token → USD/百万 token
        let to_m = |key: &str| -> Option<f64> {
            pricing.get(key).and_then(value_as_f64).map(|v| v * 1_000_000.0)
        };
        let (Some(input), Some(output)) = (to_m("prompt"), to_m("completion")) else {
            continue;
        };
        if input <= 0.0 && output <= 0.0 {
            continue;
        }
        usd.insert(
            model_id.to_lowercase(),
            ModelPrice {
                input,
                output,
                cache_read: to_m("input_cache_read").unwrap_or(0.0),
            },
        );
    }
    usd
}

/// 单条差异：用户本地与参考价格不一致的某个货币维度。
/// new_models = 参考有、用户本地没有；changed = 两边都有但三项价格不完全相同。
#[derive(Debug, Clone, Serialize)]
pub struct PriceDiffItem {
    /// 模型 id
    pub model_id: String,
    /// "cny" | "usd"
    pub currency: String,
    /// 用户当前价格（new 模型时为 None）
    pub user: Option<ModelPrice>,
    /// 参考价格
    pub default: ModelPrice,
}

/// 完整差异结果
#[derive(Debug, Clone, Serialize)]
pub struct PricingDiff {
    /// 价格来源："models.dev"（实时）| "builtin"（离线内置表）
    pub source: String,
    /// 参考表版本号（内置表的 version；models.dev 无版本概念则为空）
    pub version: String,
    /// 新增模型（参考有、用户无），默认勾选应用
    pub new_models: Vec<PriceDiffItem>,
    /// 价格变动（两边都有但不同），默认不勾选以保护用户自定义
    pub changed: Vec<PriceDiffItem>,
    /// 实际在用但「参考表与本地配置都没有价格」的模型（花费按 0 计，需手动补价）
    pub missing: Vec<String>,
}

/// 对比用户当前 pricing 与参考价，返回差异。判定"价格不同"时三项全等才算相同
/// （价格都是显式写死的配置值，用 == 比较即可）。
///
/// `relevant`: 实际调用过 ∪ 用户已配置的模型 id —— **遍历主体**，
/// 保证实际在用但参考表没收录的模型也能以 missing 暴露出来。
/// `fx_rate`: USD→CNY 汇率（models.dev 模式下 CNY 参考价 = USD × 汇率；≤0 时按 7.2 兜底）。
/// `mode`: models.dev 拉取模式（CacheOnly 不联网 / Cached 默认 / Force 强制刷新）。
pub fn diff_pricing(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
    fx_rate: f64,
    mode: FetchMode,
) -> PricingDiff {
    let fx_rate = if fx_rate > 0.0 { fx_rate } else { 7.2 };
    // 参考价：优先 models.dev（USD），失败回退内置表（USD+CNY）
    match fetch_models_dev_prices(mode) {
        Ok(usd) => {
            // CNY 参考价 = USD × 汇率（models.dev 只有国际站 USD 价）
            let cny = usd
                .iter()
                .map(|(k, p)| {
                    (
                        k.clone(),
                        ModelPrice {
                            input: p.input * fx_rate,
                            output: p.output * fx_rate,
                            cache_read: p.cache_read * fx_rate,
                        },
                    )
                })
                .collect();
            diff_with_reference(user, relevant, &usd, &cny, "models.dev", "")
        }
        Err(_) => {
            let d = load_defaults();
            diff_with_reference(user, relevant, &d.usd, &d.cny, "builtin", &d.version)
        }
    }
}

/// 纯对比逻辑（与网络解耦，便于测试）：参考价 map 的 key 需为小写模型 id。
fn diff_with_reference(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
    ref_usd: &BTreeMap<String, ModelPrice>,
    ref_cny: &BTreeMap<String, ModelPrice>,
    source: &str,
    version: &str,
) -> PricingDiff {
    // 小写归一：db 里的 model_id 大小写可能与参考表不一致（models.dev 全小写）
    let lookup = |map: &BTreeMap<String, ModelPrice>, id: &str| -> Option<ModelPrice> {
        map.get(&id.to_lowercase()).cloned()
    };

    let mut new_models = Vec::new();
    let mut changed = Vec::new();
    let mut missing = Vec::new();
    let mut relevant_sorted: Vec<&String> = relevant.iter().collect();
    relevant_sorted.sort();

    for model_id in relevant_sorted {
        let ref_u = lookup(ref_usd, model_id);
        let ref_c = lookup(ref_cny, model_id);
        let has_user_cny = user.cny.contains_key(model_id);
        let has_user_usd = user.usd.contains_key(model_id);

        if ref_u.is_none() && ref_c.is_none() {
            // 参考表完全没收录：本地也没配 → missing（花费按 0，最该提醒）
            if !has_user_cny && !has_user_usd {
                missing.push(model_id.clone());
            }
            // 本地已配 → 用户自定义价格，无参考可比，不打扰
            continue;
        }

        for (cur_name, default, user_map, has_user) in [
            ("cny", ref_c.clone(), &user.cny, has_user_cny),
            ("usd", ref_u.clone(), &user.usd, has_user_usd),
        ] {
            let Some(default) = default else { continue };
            match has_user.then(|| user_map.get(model_id).cloned()).flatten() {
                None => {
                    new_models.push(PriceDiffItem {
                        model_id: model_id.clone(),
                        currency: cur_name.to_string(),
                        user: None,
                        default,
                    });
                }
                Some(user_price) => {
                    let same = user_price.input == default.input
                        && user_price.output == default.output
                        && user_price.cache_read == default.cache_read;
                    if !same {
                        changed.push(PriceDiffItem {
                            model_id: model_id.clone(),
                            currency: cur_name.to_string(),
                            user: Some(user_price),
                            default,
                        });
                    }
                }
            }
        }
    }

    PricingDiff {
        source: source.to_string(),
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

    /// models.dev api.json 全厂商提取：逐模型容错 + 字符串/数值兼容 + 官方厂商优先
    #[test]
    fn extract_models_dev_prices() {
        let json = r#"{
            "openai": {
                "name": "OpenAI",
                "models": { "gpt-4o": { "cost": { "input": 2.5, "output": 10 } } }
            },
            "zhipuai": {
                "name": "Zhipu AI",
                "models": {
                    "glm-4.6": { "cost": { "input": 0.35, "output": 1.4, "cache_read": 0.035, "cache_write": 0.05 } },
                    "glm-4.7": { "cost": { "input": "0.2", "output": "0.8" } },
                    "broken": { "cost": { "input": 1 } },
                    "nocost": { "limit": { "context": 128000 } },
                    "zeroprice": { "cost": { "input": 0, "output": 0 } }
                }
            },
            "alibaba": {
                "name": "Alibaba",
                "models": { "qwen3.8-max": { "cost": { "input": 1.2, "output": 6 } } }
            },
            "someaggregator": {
                "name": "Cheap Router",
                "models": {
                    "glm-4.6": { "cost": { "input": 0.99, "output": 2.5 } },
                    "deepseek-v3": { "cost": { "input": 0.27, "output": 1.1 } }
                }
            }
        }"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        let usd = extract_all_prices(&root);
        // gpt-4o + glm-4.6 + glm-4.7 + qwen3.8-max + deepseek-v3（聚合商独有）
        assert_eq!(usd.len(), 5, "全厂商提取，含聚合商独有的 deepseek-v3");
        let g46 = &usd["glm-4.6"];
        assert_eq!(g46.input, 0.35, "同名模型官方厂商价优先于聚合商价");
        assert_eq!(g46.output, 1.4);
        assert_eq!(g46.cache_read, 0.035); // cache_write 不进 ModelPrice
        let g47 = &usd["glm-4.7"];
        assert_eq!(g47.input, 0.2, "字符串价格应可解析");
        assert_eq!(g47.cache_read, 0.0, "缺 cache_read 按 0");
        let qwen = &usd["qwen3.8-max"];
        assert_eq!(qwen.input, 1.2, "非智谱厂商（阿里）也应被提取");
        let ds = &usd["deepseek-v3"];
        assert_eq!(ds.input, 0.27, "聚合商独有的模型保留聚合商价");
    }

    /// jsDelivr 镜像 models.json 视图：全厂商前缀剥离 + USD/token 字符串 → USD/百万 token
    #[test]
    fn extract_from_models_json_view() {
        let json = r#"{
            "data": [
                { "id": "z-ai/glm-4.6", "pricing": { "prompt": "0.00000043", "completion": "0.00000174", "input_cache_read": "0.00000008" } },
                { "id": "z-ai/glm-4.5-air:free", "pricing": { "prompt": "0", "completion": "0" } },
                { "id": "openai/gpt-4o", "pricing": { "prompt": "0.0000025", "completion": "0.00001" } },
                { "id": "alibaba/qwen3.8-max", "pricing": { "prompt": "0.0000012", "completion": "0.000006" } },
                { "id": "z-ai/glm-4.7", "pricing": {} }
            ]
        }"#;
        let root: serde_json::Value = serde_json::from_str(json).unwrap();
        let usd = extract_from_models_json(&root);
        assert_eq!(usd.len(), 3, "glm-4.6 / gpt-4o / qwen3.8-max（free 变体与无价格跳过）");
        let g = &usd["glm-4.6"];
        assert_eq!(g.input, 0.43, "USD/token × 1M");
        assert_eq!(g.output, 1.74);
        assert_eq!(g.cache_read, 0.08);
        assert_eq!(usd["gpt-4o"].input, 2.5, "多厂商前缀均应剥离");
        assert_eq!(usd["qwen3.8-max"].input, 1.2, "阿里模型（ZCode 可接入非智谱模型）");
    }

    /// diff 分类：以 relevant 为主体 —— new / changed / missing 三类 + 大小写归一
    #[test]
    fn diff_classifies_new_changed_missing() {
        let mut user = PricingConfig::default();
        // glm-4.5 两货币都有，且 USD 价与参考一致（不产出条目）；CNY 价不同（changed）
        user.cny.insert("glm-4.5".into(), price(2.0, 8.0, 0.2));
        user.usd.insert("glm-4.5".into(), price(0.3, 1.3, 0.028));
        // glm-4-plus 已配置但参考表没有 → 自定义，不打扰（不在 missing）
        user.cny.insert("glm-4-plus".into(), price(5.0, 5.0, 0.5));

        let relevant: std::collections::HashSet<String> = [
            "glm-4.5".to_string(),
            "GLM-4.6".to_string(), // 数据库大写 → 小写归一匹配参考表
            "glm-x-new".to_string(), // 实际在用但参考表与本地都无 → missing
            "glm-4-plus".to_string(),
        ]
        .into_iter()
        .collect();

        let ref_usd = BTreeMap::from([
            ("glm-4.5".to_string(), price(0.3, 1.3, 0.028)),
            ("glm-4.6".to_string(), price(0.35, 1.4, 0.035)),
        ]);
        let ref_cny = BTreeMap::from([
            ("glm-4.5".to_string(), price(2.1, 9.1, 0.2)),
            ("glm-4.6".to_string(), price(2.5, 10.0, 0.25)),
        ]);

        let diff = diff_with_reference(&user, &relevant, &ref_usd, &ref_cny, "models.dev", "");

        // glm-4.6：USD/CNY 都是新增（保留 db 原始 id）
        assert_eq!(diff.new_models.len(), 2);
        assert!(diff
            .new_models
            .iter()
            .all(|i| i.model_id == "GLM-4.6"));
        // glm-4.5：仅 CNY 变动（USD 一致）
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].currency, "cny");
        // glm-x-new：missing；glm-4-plus：自定义不算 missing
        assert_eq!(diff.missing, vec!["glm-x-new".to_string()]);
        assert_eq!(diff.source, "models.dev");
    }

    /// 真实镜像数据管道验证（需网络，手动 `cargo test -- --ignored` 执行）
    #[test]
    #[ignore = "需要网络"]
    fn fetch_real_jsdelivr_mirror() {
        let usd = fetch_models_dev_from(
            "https://cdn.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json",
        )
        .expect("jsDelivr 镜像应可达且解析出模型价格");
        assert!(
            usd.contains_key("glm-4.6"),
            "应包含 glm-4.6，实际: {:?}",
            usd.keys().collect::<Vec<_>>()
        );
        println!("glm-4.6 = {:?}", usd["glm-4.6"]);
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

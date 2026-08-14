use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

/// 单个模型的三项单价（每百万 token）。各货币各存一份。
/// 注：input_tokens 已包含 cache_read_tokens，计费时缓存读部分单独按缓存价计算，
/// 因此非缓存输入 = input_tokens - cache_read_tokens。
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
    /// 上次成功的数据源 URL（下次优先直连，避免每次都等失败源的超时）
    #[serde(default)]
    url: String,
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

/// 价格数值取整到 6 位小数（单位：每百万 token），消除浮点乘法尾巴
/// （如 0.4 × 7.2 = 2.8800000000000003），保证展示与落盘干净。
fn round_price(x: f64) -> f64 {
    (x * 1e6).round() / 1e6
}

/// models.dev 拉取模式
pub enum FetchMode {
    /// 优先本地缓存（不管过期与否），完全无缓存才联网兜底。
    /// 「检查价格更新」按钮与进面板静默检查用——保证秒回；缓存保鲜交给每日定时任务/「更新」按钮。
    LocalFirst,
    /// 缓存未过期直接跳过；过期联网刷新（后台每日定时任务用，保证每天更新一次）
    Cached,
    /// 绕过缓存强制联网（「更新」按钮：手动拉最新数据刷缓存）
    Force,
}

/// 拉取 models.dev 全厂商模型 USD 价格（每百万 token）。
/// 磁盘缓存 24h；网络失败时回退过期缓存（宁用昨日数据也不用静态内置表），
/// 再失败返回 Err（调用方用内置表兜底）。
pub fn fetch_models_dev_prices(mode: FetchMode) -> Result<BTreeMap<String, ModelPrice>, String> {
    let cached = load_models_dev_cache();
    if !matches!(mode, FetchMode::Force) {
        if matches!(mode, FetchMode::LocalFirst) {
            // 本地优先：有缓存就用（哪怕已过期——定时任务与「更新」按钮负责保鲜）
            if let Some(c) = cached.as_ref() {
                if !c.usd.is_empty() {
                    return Ok(c.usd.clone());
                }
            }
        } else if let Some(c) = cached.as_ref() {
            // Cached（定时任务）：TTL 内不联网，过期才刷新
            if now_ms().saturating_sub(c.fetched_at) < MODELS_DEV_TTL_MS {
                return Ok(c.usd.clone());
            }
        }
    }

    // 多源 failover：官方源国内可能直连不通，依次尝试镜像。
    // 源记忆：上次成功的源排最前，常规情况下无需再等失败源的超时。
    let preferred = cached
        .as_ref()
        .map(|c| c.url.clone())
        .unwrap_or_default();
    let mut last_err = String::new();
    let mut usd = BTreeMap::new();
    let mut ok_url = String::new();
    for url in models_dev_urls(&preferred) {
        match fetch_models_dev_from(&url) {
            Ok(prices) => {
                usd = prices;
                ok_url = url;
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

    // 写缓存（失败静默：缓存只是优化），记录本次成功的数据源供下次直连
    let cache = ModelsDevCache {
        fetched_at: now_ms(),
        usd: usd.clone(),
        url: ok_url,
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
/// `preferred` 非空时（上次成功的源）挪到最前，直连命中即可省去失败源的超时等待。
///
/// 注：镜像的 models.json 是 canonical 视图（{data:[{id:"z-ai/glm-4.6", pricing:{...}}]}，
/// 价格为 USD/token 字符串），与 api.json 的 provider 视图结构不同，解析时自动分派。
fn models_dev_urls(preferred: &str) -> Vec<String> {
    let mut urls = vec![
        MODELS_DEV_URL.to_string(),
        "https://cdn.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json".to_string(),
        "https://fastly.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json".to_string(),
    ];
    if !preferred.is_empty() {
        if let Some(idx) = urls.iter().position(|u| u == preferred) {
            let hit = urls.remove(idx);
            urls.insert(0, hit);
        }
    }
    urls
}

/// 从单个 URL 拉取并提取全厂商价格（超时 6s，按响应结构自动分派视图）
fn fetch_models_dev_from(url: &str) -> Result<BTreeMap<String, ModelPrice>, String> {
    let agent = ureq::AgentBuilder::new()
        .timeout(std::time::Duration::from_secs(6))
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
        // USD/token → USD/百万 token（取整去浮点尾巴：4e-7×1e6 会得 0.39999999999999997）
        let to_m = |key: &str| -> Option<f64> {
            pricing
                .get(key)
                .and_then(value_as_f64)
                .map(|v| round_price(v * 1_000_000.0))
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

/// 单条差异（模型级）：判定基准 = 参考表 USD 原始价。
/// new_models = 参考有、用户两种货币都未配置；changed = 用户已配 USD 但三项不等。
///
/// CNY 参考价不参与"是否变动"的判定，仅作为折算展示值随条目带出
/// （models.dev 模式 = USD × 当日汇率；内置表 = 官方国内人民币价）：
/// 汇率每日自动更新，若让 CNY 折算值参与相等性判定，用户应用过的价格
/// 在汇率一动后必然被判"变动"，造成永无止境的误报。
#[derive(Debug, Clone, Serialize)]
pub struct PriceDiffItem {
    /// 模型 id
    pub model_id: String,
    /// 用户当前 USD 价格（新增模型时为 None）
    pub user: Option<ModelPrice>,
    /// 参考 USD 价格（每百万 token）
    pub default: ModelPrice,
    /// 参考 CNY 价格（应用时与 USD 一并写入）
    pub default_cny: ModelPrice,
}

/// 完整差异结果
#[derive(Debug, Clone, Serialize)]
pub struct PricingDiff {
    /// 价格来源："models.dev"（实时）| "builtin"（离线内置表）
    pub source: String,
    /// 参考表版本号（内置表的 version；models.dev 无版本概念则为空）
    pub version: String,
    /// 新增模型（参考有、用户两种货币均无），默认勾选应用（USD+CNY 一并写入）
    pub new_models: Vec<PriceDiffItem>,
    /// USD 价格变动（用户已配 USD 但与参考不同），默认不勾选以保护用户自定义
    pub changed: Vec<PriceDiffItem>,
    /// 实际在用但「参考表与本地配置都没有价格」的模型（花费按 0 计，需手动补价）
    pub missing: Vec<String>,
}

/// 对比用户当前 pricing 与参考价，返回差异。判定"是否变动"只看 USD 原始价
/// （价格都是显式写死的配置值，用 == 比较即可）；CNY 参考价 = USD × 汇率
/// （models.dev 模式）或内置表官方国内价，仅作折算展示（default_cny），
/// 应用时与 USD 一并写入——汇率每日自动变化也不会再产生误报。
///
/// `relevant`: 实际调用过 ∪ 用户已配置的模型 id —— **遍历主体**，
/// 保证实际在用但参考表没收录的模型也能以 missing 暴露出来。
/// `fx_rate`: USD→CNY 汇率（仅用于折算 default_cny；≤0 时按 7.2 兜底）。
/// `mode`: models.dev 拉取模式（LocalFirst 优先本地缓存 / Cached 过期才联网 / Force 强制刷新）。
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
            // CNY 参考价 = USD × 汇率（models.dev 只有国际站 USD 价）。
            // 取整到 6 位小数：避免 0.4×7.2=2.8800000000000003 这类浮点尾巴进配置
            let cny = usd
                .iter()
                .map(|(k, p)| {
                    (
                        k.clone(),
                        ModelPrice {
                            input: round_price(p.input * fx_rate),
                            output: round_price(p.output * fx_rate),
                            cache_read: round_price(p.cache_read * fx_rate),
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
/// 判定基准 = 参考表 USD 原始价，CNY 参考（折算值）不参与相等性判定。
fn diff_with_reference(
    user: &PricingConfig,
    relevant: &std::collections::HashSet<String>,
    ref_usd: &BTreeMap<String, ModelPrice>,
    ref_cny: &BTreeMap<String, ModelPrice>,
    source: &str,
    version: &str,
) -> PricingDiff {
    // 小写归一：db 里的 model_id 大小写可能与参考表不一致（models.dev 全小写）
    let ref_lookup = |map: &BTreeMap<String, ModelPrice>, id: &str| -> Option<ModelPrice> {
        map.get(&id.to_lowercase()).cloned()
    };
    // 用户配置也按小写建索引，让 db 原始大小写 id 能命中用户以另一种形态保存的价格
    let user_usd: BTreeMap<String, &ModelPrice> =
        user.usd.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();
    let user_cny: BTreeMap<String, &ModelPrice> =
        user.cny.iter().map(|(k, v)| (k.to_lowercase(), v)).collect();

    let mut new_models = Vec::new();
    let mut changed = Vec::new();
    let mut missing = Vec::new();
    let mut relevant_sorted: Vec<&String> = relevant.iter().collect();
    relevant_sorted.sort();
    // relevant 可能同时含同一模型的大小写两种形态（db 原始 id + 用户配置 key），
    // 归一去重，避免同一模型输出两条条目
    let mut seen = std::collections::HashSet::new();

    for model_id in relevant_sorted {
        let lc = model_id.to_lowercase();
        if !seen.insert(lc.clone()) {
            continue;
        }
        let user_u: Option<ModelPrice> = user_usd.get(&lc).map(|p| (*p).clone());
        let user_c = user_cny.get(&lc);

        // USD 参考缺失：CNY 参考也没有且本地未配 → missing（花费按 0，最该提醒）。
        // 仅 CNY 参考有（内置表国内人民币价模型）→ USD 无基准可比，不打扰；
        // 本地已配 → 用户自定义价格，同样不打扰
        let Some(default_usd) = ref_lookup(ref_usd, model_id) else {
            let ref_c_only = ref_lookup(ref_cny, model_id).is_some();
            if !ref_c_only && user_u.is_none() && user_c.is_none() {
                missing.push(model_id.clone());
            }
            continue;
        };
        // CNY 参考价缺失时以 0 兜底（正常两条参考链路同源，均有值）
        let default_cny = ref_lookup(ref_cny, model_id).unwrap_or_default();
        let item = |user: Option<ModelPrice>| PriceDiffItem {
            model_id: model_id.clone(),
            user,
            default: default_usd.clone(),
            default_cny: default_cny.clone(),
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
            // 未配 USD：CNY 也未配 → 新增模型（默认勾选，应用时 USD+CNY 一并写入）；
            // 仅配了 CNY → 视为用户已按自己的口径定价（如国内站人民币价），不打扰
            None => {
                if user_c.is_none() {
                    new_models.push(item(None));
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

    /// 源记忆：上次成功的源排最前；未知源/空值保持默认顺序
    #[test]
    fn models_dev_urls_prefers_last_success() {
        let mirror = "https://cdn.jsdelivr.net/gh/anomalyco/models.dev@dev/models.json";
        let urls = models_dev_urls(mirror);
        assert_eq!(urls[0], mirror, "上次成功的镜像源应排第一");
        assert_eq!(urls[1], MODELS_DEV_URL, "官方源退居第二");

        // 未知源（缓存被手改等）：不 panic，维持默认顺序
        assert_eq!(models_dev_urls("https://unknown.example/api.json")[0], MODELS_DEV_URL);
        // 无记忆
        assert_eq!(models_dev_urls("")[0], MODELS_DEV_URL);
    }

    /// round_price：消除浮点尾巴（换算 USD×汇率 / USD/token×1M 场景）
    #[test]
    fn round_price_trims_float_tail() {
        assert_eq!(round_price(0.4 * 7.2), 2.88); // 2.8800000000000003 → 2.88
        assert_eq!(round_price(4e-7 * 1e6), 0.4); // 0.39999999999999997 → 0.4
        assert_eq!(round_price(0.11), 0.11); // 正常值不变
        assert_eq!(round_price(10.0), 10.0);
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

    /// diff 分类：判定只看 USD 原始价 —— new / changed / missing + 大小写归一
    #[test]
    fn diff_classifies_new_changed_missing() {
        let mut user = PricingConfig::default();
        // glm-4.5：USD 与参考一致 → 无条目。CNY 与折算参考不同也不算变动
        //（折算值随汇率每日漂移，参与判定会造成持续误报）
        user.usd.insert("glm-4.5".into(), price(0.3, 1.3, 0.028));
        user.cny.insert("glm-4.5".into(), price(2.0, 8.0, 0.2));
        // glm-4-air：USD 与参考不同 → changed
        user.usd.insert("glm-4-air".into(), price(0.1, 0.4, 0.01));
        // glm-4-plus：仅配 CNY（国内站人民币自定义价）→ 参考有 USD 也不打扰
        user.cny.insert("glm-4-plus".into(), price(5.0, 5.0, 0.5));
        // gpt-5：用户小写配置 + relevant 混入 db 大写形态，归一后 USD 一致 → 无条目
        user.usd.insert("gpt-5".into(), price(1.25, 10.0, 0.125));

        let relevant: std::collections::HashSet<String> = [
            "glm-4.5".to_string(),
            "GLM-4.6".to_string(), // 数据库大写，两货币均无 → 新增（保留 db 原始 id）
            "glm-4-air".to_string(),
            "glm-4-plus".to_string(),
            "GPT-5".to_string(), // 与用户小写 key 归一去重
            "gpt-5".to_string(),
            "glm-x-new".to_string(), // 实际在用但参考表与本地都无 → missing
            "glm-4-cny-only".to_string(), // 参考仅 CNY（国内价）→ 不打扰、不 missing
        ]
        .into_iter()
        .collect();

        let ref_usd = BTreeMap::from([
            ("glm-4.5".to_string(), price(0.3, 1.3, 0.028)),
            ("glm-4.6".to_string(), price(0.35, 1.4, 0.035)),
            ("glm-4-air".to_string(), price(0.11, 0.42, 0.011)),
            ("glm-4-plus".to_string(), price(0.7, 2.8, 0.07)),
            ("gpt-5".to_string(), price(1.25, 10.0, 0.125)),
        ]);
        let ref_cny = BTreeMap::from([
            ("glm-4.6".to_string(), price(2.5, 10.0, 0.25)), // glm-4-air 故意缺失，测 0 兜底
            // cny-only：内置表仅有国内人民币价的模型（USD 参考缺失）
            ("glm-4-cny-only".to_string(), price(0.6, 2.2, 0.06)),
        ]);

        let diff = diff_with_reference(&user, &relevant, &ref_usd, &ref_cny, "models.dev", "");

        // 新增：仅 glm-4.6（gpt-5 归一后已配置；glm-4-plus 仅 CNY 不打扰）
        assert_eq!(diff.new_models.len(), 1);
        assert_eq!(diff.new_models[0].model_id, "GLM-4.6");
        assert_eq!(
            diff.new_models[0].default_cny,
            price(2.5, 10.0, 0.25),
            "新增条目应携带 CNY 折算参考价"
        );
        // 变动：仅 glm-4-air（USD 三项不等）
        assert_eq!(diff.changed.len(), 1);
        assert_eq!(diff.changed[0].model_id, "glm-4-air");
        assert_eq!(diff.changed[0].user, Some(price(0.1, 0.4, 0.01)));
        assert_eq!(
            diff.changed[0].default_cny,
            ModelPrice::default(),
            "参考 CNY 缺失时以 0 兜底"
        );
        // missing：glm-x-new；glm-4-plus 已配 CNY、glm-4-cny-only 参考有国内价，均不算 missing
        assert_eq!(diff.missing, vec!["glm-x-new".to_string()]);
        assert_eq!(diff.source, "models.dev");
    }

    /// apply_updates 写入前收敛同模型大小写重复 key：
    /// 否则归一索引取旧值、应用写入新形态，changed 条目应用后仍不清零
    #[test]
    fn collapse_insert_removes_case_variants() {
        let mut map = BTreeMap::from([
            ("glm-4.6".to_string(), price(0.3, 1.3, 0.028)), // 手输小写旧值
            ("gpt-5".to_string(), price(1.25, 10.0, 0.125)), // 无关模型，应保留
        ]);
        collapse_insert(&mut map, "GLM-4.6", price(0.35, 1.4, 0.035));
        assert_eq!(map.len(), 2, "同模型应收敛为单一 key");
        assert_eq!(map.get("GLM-4.6"), Some(&price(0.35, 1.4, 0.035)));
        assert!(map.get("glm-4.6").is_none(), "旧小写形态应被清除");
        assert!(map.contains_key("gpt-5"), "其他模型不受影响");
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
        collapse_insert(map, model_id, price.clone());
    }
    save_pricing(&cfg)?;
    Ok(cfg)
}

/// 删除同模型其他大小写形态的旧 key 后写入（收敛为单一 key）。
/// 如手输的 "glm-4.6" 与应用写入的 "GLM-4.6" 并存时，diff 的归一索引会取到
/// 旧值，导致应用后 changed 条目仍不清零、红点反复出现。
fn collapse_insert(map: &mut BTreeMap<String, ModelPrice>, model_id: &str, price: ModelPrice) {
    let lc = model_id.to_lowercase();
    map.retain(|k, _| k.to_lowercase() != lc);
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

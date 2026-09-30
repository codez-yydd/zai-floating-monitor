//! 网络代理配置（~/.zbar/proxy.json）：
//! models.dev 官方价格同步与汇率更新共用的出站 HTTP 代理。
//! 空串 = 直连（与未配置行为一致）。

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;
use std::time::Duration;

/// 代理配置文件结构：{ "proxy": "http://127.0.0.1:7890" }
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// 代理地址，支持 http/socks5（socks4/socks4a 亦可用）；https 协议的代理
    /// 服务器 ureq2 不支持，构建时会被拦截提示。空串 = 直连
    #[serde(default)]
    pub proxy: String,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            proxy: String::new(),
        }
    }
}

/// ~/.zbar/proxy.json 路径（配置目录复用 pricing.rs，全应用配置集中一处）
fn proxy_config_path() -> Result<PathBuf, String> {
    Ok(crate::pricing::config_dir()?.join("proxy.json"))
}

/// 读取代理配置；文件不存在返回默认空配置（直连，不报错，与 pricing.json 同口径）。
/// 文件损坏时同样回退默认直连并记日志（与 shortcut.rs 容错口径一致）：设置页
/// Promise.all 聚合加载，这里报错会让整页打不开、用户失去从 UI 修复的机会
pub fn load_proxy_config() -> Result<ProxyConfig, String> {
    let path = proxy_config_path()?;
    if !path.exists() {
        return Ok(ProxyConfig::default());
    }
    let data = fs::read_to_string(&path).map_err(|e| format!("读取代理配置失败: {e}"))?;
    serde_json::from_str::<ProxyConfig>(&data).or_else(|e| {
        eprintln!("[zbar-net] 代理配置文件损坏，回退默认直连（重新保存即可覆盖修复）: {e}");
        Ok(ProxyConfig::default())
    })
}

/// 保存代理配置。入参去首尾空白，避免粘贴时带入的换行/空格让代理解析失败。
pub fn save_proxy_config(proxy: &str) -> Result<(), String> {
    let dir = crate::pricing::config_dir()?;
    fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let cfg = ProxyConfig {
        proxy: proxy.trim().to_string(),
    };
    let data =
        serde_json::to_string_pretty(&cfg).map_err(|e| format!("序列化代理配置失败: {e}"))?;
    fs::write(proxy_config_path()?, data).map_err(|e| format!("写入代理配置失败: {e}"))
}

/// 构建出站 HTTP Agent：配置了代理则经代理（http/socks5 等，ureq 已启用
/// socks-proxy 特性），为空则直连。timeout_secs 为读取超时（连接超时单独收紧）；
/// ureq2 的超时只能在构建期配置，故按调用方超时口径参数化
/// （models.dev 同步 30s、汇率拉取 15s 各自传入）。
/// 代理串非法时返回中文错误，由调用方决定是否降级。
pub fn http_agent(timeout_secs: u64) -> Result<ureq::Agent, String> {
    let proxy = load_proxy_config()?.proxy;
    // 保存时已 trim，这里再防御一次（手工改配置文件的场景）
    build_agent(proxy.trim(), timeout_secs)
}

/// 按给定代理串构建 Agent（纯函数，便于单测不触碰真实配置文件）。
fn build_agent(proxy: &str, timeout_secs: u64) -> Result<ureq::Agent, String> {
    let builder = ureq::AgentBuilder::new()
        // 连接失败尽早暴露：代理地址写错时 10s 内报错，不等满读取超时
        .timeout_connect(Duration::from_secs(10))
        .timeout_read(Duration::from_secs(timeout_secs))
        .timeout_write(Duration::from_secs(timeout_secs));
    if proxy.is_empty() {
        return Ok(builder.build());
    }
    // ureq2 的 Proxy 不支持 https 协议的代理服务器（无法与代理本身建 TLS），
    // 用户常从 Clash/V2Ray 抄到 https:// 地址，提前拦截并给出可操作的提示，
    // 避免只看到笼统的 InvalidProxyUrl。
    // 切片用 get(..8)：切点落在多字节字符（全角冒号、中文输入等）中间时
    // proxy[..8] 会 panic，且该 panic 发生在后台线程会带崩汇率刷新
    if proxy
        .get(..8)
        .map_or(false, |head| head.eq_ignore_ascii_case("https://"))
    {
        return Err(format!(
            "暂不支持 https:// 开头的代理地址（{proxy}），请改用 http:// 或 socks5://"
        ));
    }
    let p = ureq::Proxy::new(proxy).map_err(|e| format!("代理地址无效（{proxy}）: {e}"))?;
    Ok(builder.proxy(p).build())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 配置结构往返：带代理 / 空串（直连）均可序列化还原
    #[test]
    fn proxy_config_roundtrip() {
        let cfg = ProxyConfig {
            proxy: "socks5://127.0.0.1:7890".to_string(),
        };
        let s = serde_json::to_string(&cfg).unwrap();
        let back: ProxyConfig = serde_json::from_str(&s).unwrap();
        assert_eq!(back.proxy, "socks5://127.0.0.1:7890");
        // 缺 proxy 字段（旧/手写文件）回退空串 = 直连
        let bare: ProxyConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(bare.proxy, "");
    }

    /// 空代理 = 直连，Agent 构建成功
    #[test]
    fn build_agent_direct_ok() {
        let agent = build_agent("", 15);
        assert!(agent.is_ok(), "直连 Agent 构建不应失败: {agent:?}");
    }

    /// 合法的 http / socks5 代理串均可构建（仅验证解析，不发真实请求）
    #[test]
    fn build_agent_accepts_valid_proxy_strings() {
        assert!(build_agent("http://127.0.0.1:7890", 30).is_ok());
        assert!(build_agent("socks5://127.0.0.1:7890", 30).is_ok());
        // 带认证信息的形态（ureq 文档示例之一）
        assert!(build_agent("socks5://john:smith@socks.google.com:1080", 30).is_ok());
    }

    /// ureq2 不支持 https 协议的代理服务器：提前拦截并给出可操作的中文提示
    #[test]
    fn build_agent_rejects_https_proxy_scheme() {
        let err = build_agent("https://proxy.example.com:8443", 15).unwrap_err();
        assert!(
            err.contains("http:// 或 socks5://"),
            "应提示改用 http/socks5: {err}"
        );
    }

    /// 第 8 字节切在多字节字符中间时不得 panic（曾因 proxy[..8] 直接切片越界，
    /// 且 panic 发生在后台线程会带崩汇率刷新）：Ok / Err 均可，唯一要求是不 panic
    #[test]
    fn build_agent_no_panic_on_multibyte_prefix() {
        // "代理" 6 字节 + 全角冒号 3 字节：切点落在冒号中间
        let _ = build_agent("代理：127.0.0.1:7890", 15);
        // 7 个 ASCII + é（2 字节）：切点落在 é 中间
        let _ = build_agent("aaaaaaaé", 15);
    }
}

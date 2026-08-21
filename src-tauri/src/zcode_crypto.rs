//! ZCode 客户端凭证解密（~/.zcode/v2/credentials.json 的 `enc:v1:` 格式）。
//!
//! 算法与 ZCode 客户端一致（社区逆向结论，参考实现 Apache 2.0）：
//! - AES-256-GCM，key = SHA256(secret)
//! - secret = 环境变量 `ZCODE_CREDENTIAL_SECRET`；未设置时使用
//!   `zcode-credential-fallback:{platform}:{home}:{username}` 兜底拼接
//! - 密文格式：`enc:v1:<nonce_b64url>.<tag_b64url>.<cipher_b64url>`，
//!   base64url 无 padding（解密兼容带 padding 的变体）
//! - aes-gcm crate 惯例：解密时把 tag 拼接到 cipher 尾部一起传入
//!
//! 本模块只做纯函数解析/解密，不做任何文件 IO；调用方见 accounts.rs。
//! 注意：额度查询（quota.rs）不依赖本模块，两者互不影响。

use aes_gcm::aead::generic_array::GenericArray;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::Aes256Gcm;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine as _;
use serde_json::Value;
use sha2::{Digest, Sha256};

/// 凭证密文前缀（非该前缀的值视为明文原样返回）。
const ENC_PREFIX: &str = "enc:v1:";

/// 账号指纹：从 JWT 中提取的用户标识，用于区分不同登录账号。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fingerprint {
    /// 用户 ID（JWT payload 的 user_id，缺失时兜底 sub）
    pub user_id: String,
    /// 邮箱（JWT 或 user_info 中可能缺失，均可为 None）
    pub email: Option<String>,
    /// 昵称（user_info 的 displayName，缺失时依次回落 username / name）
    pub display_name: Option<String>,
}

/// 读取凭证加密 secret（UTF-8 字节）。优先环境变量 `ZCODE_CREDENTIAL_SECRET`，
/// 未设置时按平台兜底拼接。返回原文 secret（尚未做 SHA256）。
pub fn credential_secret() -> Vec<u8> {
    if let Ok(env_secret) = std::env::var("ZCODE_CREDENTIAL_SECRET") {
        if !env_secret.is_empty() {
            return env_secret.into_bytes();
        }
    }
    let platform = if cfg!(target_os = "macos") {
        "darwin"
    } else if cfg!(target_os = "windows") {
        "win32"
    } else {
        "linux"
    };
    let home = dirs::home_dir()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let username = std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_default();
    fallback_secret(platform, &home, &username)
}

/// 兜底 secret 拼接（纯函数，便于单测格式断言）。
fn fallback_secret(platform: &str, home: &str, username: &str) -> Vec<u8> {
    format!("zcode-credential-fallback:{platform}:{home}:{username}").into_bytes()
}

/// base64url 解码（无 padding，兼容尾部带 `=` 的输入）。
fn b64url_decode(s: &str) -> Result<Vec<u8>, String> {
    let trimmed = s.trim_end_matches('=');
    URL_SAFE_NO_PAD
        .decode(trimmed)
        .map_err(|e| format!("base64url 解码失败: {e}"))
}

/// 解密单个凭证值。
/// - 非 `enc:v1:` 前缀：视为明文，原样返回（老版本客户端存在明文键）。
/// - 格式损坏 / 密钥不匹配 / 认证失败：返回 Err。
pub fn decrypt_value(raw: &str, secret: &[u8]) -> Result<String, String> {
    let Some(body) = raw.strip_prefix(ENC_PREFIX) else {
        return Ok(raw.to_string());
    };
    let parts: Vec<&str> = body.split('.').collect();
    if parts.len() != 3 {
        return Err("凭证密文格式异常（应为 nonce.tag.cipher 三段）".into());
    }
    let nonce = b64url_decode(parts[0])?;
    let tag = b64url_decode(parts[1])?;
    let cipher = b64url_decode(parts[2])?;
    if nonce.len() != 12 {
        return Err(format!("凭证 nonce 长度异常: {}", nonce.len()));
    }
    // key = SHA256(secret) → AES-256
    let key = Sha256::digest(secret);
    let decryptor = Aes256Gcm::new(GenericArray::from_slice(&key));
    // aes-gcm crate 惯例：tag 拼到密文尾部
    let mut cipher_and_tag = cipher;
    cipher_and_tag.extend_from_slice(&tag);
    let plaintext = decryptor
        .decrypt(
            GenericArray::from_slice(&nonce),
            cipher_and_tag.as_slice(),
        )
        .map_err(|_| "凭证解密失败（认证不通过，secret 可能不匹配）".to_string())?;
    String::from_utf8(plaintext).map_err(|e| format!("凭证明文非 UTF-8: {e}"))
}

/// 解析 JWT 的 payload 段（不验签，仅用于读取 user_id/email）。
/// token 形如 `header.payload.signature`，payload 为 base64url 编码的 JSON。
pub fn jwt_payload(token: &str) -> Option<Value> {
    let parts: Vec<&str> = token.split('.').collect();
    if parts.len() < 2 {
        return None;
    }
    let bytes = b64url_decode(parts[1]).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// 从 credentials.json 内容中提取账号指纹。
/// 流程：解密 `zcodejwttoken` → 解析 JWT payload → 取 user_id（兜底 sub）；
/// email 先取 JWT 的 email 字段，缺失时再解密 `oauth:bigmodel:user_info` 取；
/// 昵称只来自 user_info（displayName，缺失依次回落 username / name）。
/// 任一环节解密/解析失败均返回 None（降级不阻塞：调用方自行决定兜底路径）。
pub fn fingerprint_of_credentials(creds: &Value) -> Option<Fingerprint> {
    let secret = credential_secret();
    let jwt_raw = creds.get("zcodejwttoken")?.as_str()?;
    let jwt = decrypt_value(jwt_raw, &secret).ok()?;
    let payload = jwt_payload(&jwt)?;
    let user_id = payload
        .get("user_id")
        .and_then(|v| v.as_str())
        .or_else(|| payload.get("sub").and_then(|v| v.as_str()))?
        .to_string();
    // user_info 只解密一次，同时取 email（JWT email 缺失时的回落）与昵称
    let (user_info_email, display_name) =
        user_info_fields(creds, &secret).unwrap_or((None, None));
    let email = payload
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .or(user_info_email);
    Some(Fingerprint {
        user_id,
        email,
        display_name,
    })
}

/// 从 `oauth:bigmodel:user_info`（解密后为 JSON）中提取 email 与昵称。
/// 一次解密同时返回两个字段（避免同一密文解密两次）；
/// 昵称仅取 displayName / username / name 三个键，按此优先级回落。
/// user_info 缺失或解密失败返回 None。
fn user_info_fields(creds: &Value, secret: &[u8]) -> Option<(Option<String>, Option<String>)> {
    let raw = creds.get("oauth:bigmodel:user_info")?.as_str()?;
    let decrypted = decrypt_value(raw, secret).ok()?;
    let info: Value = serde_json::from_str(&decrypted).ok()?;
    let email = info
        .get("email")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string());
    let display_name = ["displayName", "username", "name"]
        .iter()
        .find_map(|k| {
            info.get(*k)
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
                .map(|s| s.to_string())
        });
    Some((email, display_name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use aes_gcm::aead::rand_core::OsRng;
    use aes_gcm::AeadCore;

    /// 测试专用加密：生成与客户端一致的 `enc:v1:` 密文，用于往返验证。
    fn encrypt_value(plain: &str, secret: &[u8]) -> String {
        let key = Sha256::digest(secret);
        let decryptor = Aes256Gcm::new(GenericArray::from_slice(&key));
        let nonce = Aes256Gcm::generate_nonce(&mut OsRng);
        let cipher = decryptor
            .encrypt(&nonce, plain.as_bytes())
            .expect("测试加密不应失败");
        // cipher 尾部自带 16 字节 tag，拆成三段拼 enc:v1: 格式
        let (body, tag) = cipher.split_at(cipher.len() - 16);
        format!(
            "{ENC_PREFIX}{}.{}.{}",
            URL_SAFE_NO_PAD.encode(nonce),
            URL_SAFE_NO_PAD.encode(tag),
            URL_SAFE_NO_PAD.encode(body)
        )
    }

    /// base64url 编码（造 JWT payload 用）
    fn b64url_encode(bytes: &[u8]) -> String {
        URL_SAFE_NO_PAD.encode(bytes)
    }

    #[test]
    fn encrypt_decrypt_roundtrip() {
        let secret = b"test-secret";
        let raw = encrypt_value("hello 智谱 zcode", secret);
        assert!(raw.starts_with("enc:v1:"));
        assert_eq!(decrypt_value(&raw, secret).unwrap(), "hello 智谱 zcode");
    }

    #[test]
    fn decrypt_with_wrong_secret_fails() {
        let raw = encrypt_value("secret value", b"secret-a");
        assert!(decrypt_value(&raw, b"secret-b").is_err());
    }

    #[test]
    fn plaintext_passthrough() {
        // 非 enc:v1: 前缀的明文（如老版客户端的明文键）应原样返回
        assert_eq!(decrypt_value("plain-value", b"any").unwrap(), "plain-value");
        assert_eq!(decrypt_value("", b"any").unwrap(), "");
    }

    #[test]
    fn corrupted_ciphertext_errors() {
        let secret = b"test-secret";
        let raw = encrypt_value("data", secret);
        // 三段密文中篡改任意一段都应报错而不是返回乱码
        let body = &raw[ENC_PREFIX.len()..];
        let mut parts: Vec<String> = body.split('.').map(|s| s.to_string()).collect();
        parts[2] = format!("{}AA", parts[2]); // 篡改 cipher 段
        let corrupted = format!("{ENC_PREFIX}{}", parts.join("."));
        assert!(decrypt_value(&corrupted, secret).is_err());
        // 段数不对也报错
        assert!(decrypt_value("enc:v1:only.one", secret).is_err());
    }

    #[test]
    fn fallback_secret_format() {
        let s = fallback_secret("darwin", "/Users/alice", "alice");
        assert_eq!(
            String::from_utf8(s).unwrap(),
            "zcode-credential-fallback:darwin:/Users/alice:alice"
        );
    }

    #[test]
    fn jwt_payload_parses_without_signature_check() {
        // header.payload.signature（签名随便填，不验签）
        let payload = r#"{"user_id":"123456","sub":"123456","email":"a@b.c"}"#;
        let token = format!(
            "{}.{}.sig",
            b64url_encode(br#"{"alg":"HS256"}"#),
            b64url_encode(payload.as_bytes())
        );
        let p = jwt_payload(&token).expect("JWT payload 应解析成功");
        assert_eq!(p.get("user_id").and_then(|v| v.as_str()), Some("123456"));
        assert_eq!(p.get("email").and_then(|v| v.as_str()), Some("a@b.c"));
        // 非 JWT 形态返回 None
        assert!(jwt_payload("not-a-jwt").is_none());
    }

    #[test]
    fn fingerprint_of_credentials_extracts_user_id_and_email() {
        let secret = credential_secret();
        let payload = r#"{"user_id":"16361781344907127","sub":"16361781344907127"}"#;
        let jwt = format!(
            "{}.{}.sig",
            b64url_encode(br#"{"alg":"HS256"}"#),
            b64url_encode(payload.as_bytes())
        );
        let creds = serde_json::json!({
            "zcodejwttoken": encrypt_value(&jwt, &secret),
            "oauth:bigmodel:access_token": encrypt_value("fake-at", &secret),
        });
        let fp = fingerprint_of_credentials(&creds).expect("指纹应提取成功");
        assert_eq!(fp.user_id, "16361781344907127");
        assert_eq!(fp.email, None);
        // 无 user_info 键 → 昵称也为 None
        assert_eq!(fp.display_name, None);
    }

    #[test]
    fn fingerprint_falls_back_to_user_info_email() {
        let secret = credential_secret();
        let payload = r#"{"user_id":"u1"}"#;
        let jwt = format!(
            "{}.{}.sig",
            b64url_encode(br#"{"alg":"HS256"}"#),
            b64url_encode(payload.as_bytes())
        );
        let user_info = r#"{"id":"u1","email":"x@y.z","displayName":"小智"}"#;
        let creds = serde_json::json!({
            "zcodejwttoken": encrypt_value(&jwt, &secret),
            "oauth:bigmodel:user_info": encrypt_value(user_info, &secret),
        });
        let fp = fingerprint_of_credentials(&creds).expect("指纹应提取成功");
        assert_eq!(fp.user_id, "u1");
        assert_eq!(fp.email.as_deref(), Some("x@y.z"));
        // 昵称取 displayName（最高优先级）
        assert_eq!(fp.display_name.as_deref(), Some("小智"));
    }

    /// 昵称回落：displayName 缺失时依次取 username / name
    #[test]
    fn display_name_falls_back_to_username_then_name() {
        let secret = credential_secret();
        let make_creds = |user_info: &str| {
            let payload = r#"{"user_id":"u1"}"#;
            let jwt = format!(
                "{}.{}.sig",
                b64url_encode(br#"{"alg":"HS256"}"#),
                b64url_encode(payload.as_bytes())
            );
            serde_json::json!({
                "zcodejwttoken": encrypt_value(&jwt, &secret),
                "oauth:bigmodel:user_info": encrypt_value(user_info, &secret),
            })
        };
        // displayName 缺失 → username
        let fp = fingerprint_of_credentials(&make_creds(
            r#"{"email":"a@b.c","username":"u-name","name":"n-name"}"#,
        ))
        .expect("指纹应提取成功");
        assert_eq!(fp.display_name.as_deref(), Some("u-name"));
        // displayName / username 都缺失 → name
        let fp = fingerprint_of_credentials(&make_creds(
            r#"{"email":"a@b.c","name":"n-name"}"#,
        ))
        .expect("指纹应提取成功");
        assert_eq!(fp.display_name.as_deref(), Some("n-name"));
        // 三个键都缺失 → None（email 逻辑不受影响）
        let fp = fingerprint_of_credentials(&make_creds(r#"{"email":"a@b.c"}"#))
            .expect("指纹应提取成功");
        assert_eq!(fp.display_name, None);
        assert_eq!(fp.email.as_deref(), Some("a@b.c"));
    }

    /// JWT 自带 email 时仍会解析 user_info 取昵称（email 优先级不变）
    #[test]
    fn display_name_parsed_even_with_jwt_email() {
        let secret = credential_secret();
        let payload = r#"{"user_id":"u1","email":"jwt@b.c"}"#;
        let jwt = format!(
            "{}.{}.sig",
            b64url_encode(br#"{"alg":"HS256"}"#),
            b64url_encode(payload.as_bytes())
        );
        let creds = serde_json::json!({
            "zcodejwttoken": encrypt_value(&jwt, &secret),
            "oauth:bigmodel:user_info": encrypt_value(
                r#"{"email":"info@b.c","displayName":"昵称"}"#, &secret
            ),
        });
        let fp = fingerprint_of_credentials(&creds).expect("指纹应提取成功");
        assert_eq!(fp.email.as_deref(), Some("jwt@b.c"));
        assert_eq!(fp.display_name.as_deref(), Some("昵称"));
    }

    #[test]
    fn fingerprint_returns_none_on_decrypt_failure() {
        // 用错误的 secret 加密（模拟换机器后 secret 变化），指纹降级为 None
        let payload = r#"{"user_id":"u1"}"#;
        let jwt = format!(
            "{}.{}.sig",
            b64url_encode(br#"{"alg":"HS256"}"#),
            b64url_encode(payload.as_bytes())
        );
        let creds = serde_json::json!({
            "zcodejwttoken": encrypt_value(&jwt, b"wrong-secret"),
        });
        // 当前环境的 credential_secret 与 wrong-secret 不同 → 解密失败 → None。
        // （若环境恰好设置了 ZCODE_CREDENTIAL_SECRET=wrong-secret 则跳过该断言）
        if std::env::var("ZCODE_CREDENTIAL_SECRET").as_deref() != Ok("wrong-secret") {
            assert!(fingerprint_of_credentials(&creds).is_none());
        }
        // 缺 key 也返回 None
        assert!(fingerprint_of_credentials(&serde_json::json!({})).is_none());
    }
}

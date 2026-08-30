//! 手动 Cookie 凭证的通用解析与浏览器仿真头工具。
//!
//! 用户在凭证弹层里可能粘贴三种形态的 Cookie 内容：
//! 1. 裸 cookie 串（`name=value; name2=value2`，可带前导 `Cookie:` 头名）；
//! 2. 浏览器「Copy as cURL」复制的整段命令（含 `-H 'Cookie: ...'` /
//!    `-H "Cookie: ..."` / `--cookie '...'` / `-b '...'`，可能出现多条
//!    Cookie 头，参考 CodexBar LongCat 的 headerPatterns 思路全部提取拼接）；
//! 3. 混杂包裹引号 / 首尾空白的上述任意形态。
//!
//! `normalize_cookie_secret` 统一归一为裸 `name=value; name2=value2` 串，
//! 供各 cookie 型 provider（qoder / longcat）直接放进请求头；解析不出任何
//! Cookie 时返回空串（调用方据此产出 error 条目提示用户重新复制）。
//! `chrome_like_headers` 生成统一的浏览器仿真头（Chrome UA / Accept /
//! Accept-Language / Origin / Referer），避免各 provider 手拼不一致被服务端
//! 风控拦截。
//!
//! 工程纪律（对齐 provider_quota.rs）：
//! - 纯函数、无正则依赖（项目未引入 regex，Copy as cURL 的固定形态用
//!   手写字符扫描覆盖即可）；单测不联网；
//! - 安全：函数只返回归一后的 Cookie 值，任何日志/错误消息都不得携带。

// ============================================================
// Cookie 内容归一
// ============================================================

/// 把用户粘贴的 Cookie 内容归一为裸 `name=value; name2=value2` 串。
/// 三种输入形态（裸串 / 整段 cURL / 混杂引号空白）统一在此收敛；
/// 解析不出任何 Cookie（如不含 Cookie 头的 cURL）返回空串，由调用方
/// 产出 error 条目（错误消息不得回显粘贴内容，可能含敏感 Cookie）。
pub(crate) fn normalize_cookie_secret(secret: &str) -> String {
    let trimmed = secret.trim();
    if trimmed.is_empty() {
        return String::new();
    }
    // 1) cURL 形态：能从 flag 参数里提取到任何 Cookie → 用提取结果拼接
    let from_curl = extract_curl_cookies(trimmed);
    if !from_curl.is_empty() {
        return normalize_pairs(&from_curl.join("; "));
    }
    // 2) 明确是 cURL 命令但没提到 Cookie → 空串（调用方报「无法解析」）
    if looks_like_curl(trimmed) {
        return String::new();
    }
    // 3) 裸 cookie 串：去包裹引号 + 容忍前导 `Cookie:` 头名
    let bare = strip_wrapping_quotes(trimmed);
    let value = strip_header_name(bare, "cookie").unwrap_or(bare);
    normalize_pairs(value)
}

/// 是否疑似 cURL 命令文本（用于「是 cURL 但没 Cookie」时显式报空，
/// 而不是把整段命令误当裸 cookie）。大小写不敏感。
fn looks_like_curl(s: &str) -> bool {
    let lower = s.to_ascii_lowercase();
    lower.starts_with("curl")
        || lower.contains("--header")
        || lower.contains("--cookie")
        || lower.contains("-h ")
        || lower.contains("-b ")
}

/// 从 cURL 文本提取所有 Cookie 头值（手动字符扫描，无正则依赖）。
/// 识别 `-H` / `--header`（值需为 `Cookie: ...` 形态）与 `--cookie` / `-b`
/// （值本身即 cookie，容忍整段 `Cookie: ...`）；多条 Cookie 头按出现顺序
/// 返回，由调用方拼接。`$'...'`（bash ANSI-C 引用）剥掉 `$` 后按引号处理。
fn extract_curl_cookies(text: &str) -> Vec<String> {
    let mut cookies: Vec<String> = Vec::new();
    // 长 flag 在前，避免短 flag 抢占前缀匹配
    const FLAGS: [&str; 4] = ["--header", "--cookie", "-H", "-b"];
    let bytes = text.as_bytes();
    let mut i = 0usize;
    while i < bytes.len() {
        // flag 均为 ASCII：多字节字符（如粘贴内容里的中文）中间不可切片，
        // 也不可能是 flag 起点，按字节推进到下一个字符边界
        if !text.is_char_boundary(i) {
            i += 1;
            continue;
        }
        // flag 必须出现在「串首或空白之后」，避免命中 URL/值中的子串
        let at_boundary = i == 0 || bytes[i - 1].is_ascii_whitespace();
        if !at_boundary {
            i += 1;
            continue;
        }
        let rest = &text[i..];
        let matched = FLAGS.iter().find(|f| {
            rest.starts_with(**f)
                && rest[(*f).len()..]
                    .chars()
                    .next()
                    .map(char::is_whitespace)
                    .unwrap_or(false)
        });
        let Some(flag) = matched else {
            i += 1;
            continue;
        };
        let after = &rest[flag.len()..];
        let (value, consumed) = read_arg(after);
        let value = value.trim();
        let is_header_flag = *flag == "-H" || *flag == "--header";
        let extracted = if is_header_flag {
            // -H 'Cookie: a=1; b=2' → 取头名后的值；非 Cookie 头跳过
            strip_header_name(value, "cookie").map(str::to_string)
        } else {
            // --cookie 'a=1' / -b 'a=1'：值即 cookie，容忍误带 `Cookie:` 头名
            Some(
                strip_header_name(value, "cookie")
                    .unwrap_or(value)
                    .to_string(),
            )
        };
        if let Some(v) = extracted {
            if !v.trim().is_empty() {
                cookies.push(v);
            }
        }
        i += flag.len() + consumed;
    }
    cookies
}

/// 读一个 shell 风格参数（从入参开头算，含前导空白）：引号包裹读到配对
/// 引号，否则读到第一个空白。返回 (参数内容, 消耗的字节数)。
fn read_arg(s: &str) -> (String, usize) {
    let trimmed = s.trim_start();
    let prefix = s.len() - trimmed.len();
    let mut chars = trimmed.chars();
    let Some(first) = chars.next() else {
        return (String::new(), s.len());
    };
    if first == '$' {
        // bash $'...' / $"..."：剥掉 $ 后按引号参数递归处理
        let (v, c) = read_arg(&trimmed[1..]);
        return (v, prefix + 1 + c);
    }
    if first == '\'' || first == '"' {
        // 引号包裹：读到下一个同种引号（Copy as cURL 不转义引号，够用）；
        // 未闭合时读到串尾
        let close = trimmed[1..].find(first).map(|i| i + 1);
        return match close {
            Some(end) => (trimmed[1..end].to_string(), prefix + end + 1),
            None => (trimmed[1..].to_string(), s.len()),
        };
    }
    // 无引号：读到第一个空白
    let end = trimmed.find(char::is_whitespace).unwrap_or(trimmed.len());
    (trimmed[..end].to_string(), prefix + end)
}

/// 若值形如 `Cookie: xxx` / `cookie:xxx`（头名大小写不敏感），取冒号后的
/// 部分；不是该头名返回 None。
fn strip_header_name<'a>(value: &'a str, name: &str) -> Option<&'a str> {
    let idx = value.find(':')?;
    if value[..idx].trim().eq_ignore_ascii_case(name) {
        Some(value[idx + 1..].trim())
    } else {
        None
    }
}

/// 去最外层成对引号（可叠多层，如 `'a=1'`、`"'a=1'"`）；非成对不动。
fn strip_wrapping_quotes(s: &str) -> &str {
    let mut s = s.trim();
    while s.len() >= 2 {
        let b = s.as_bytes();
        let paired = (b[0] == b'\'' && b[s.len() - 1] == b'\'')
            || (b[0] == b'"' && b[s.len() - 1] == b'"');
        if !paired {
            break;
        }
        s = s[1..s.len() - 1].trim();
    }
    s
}

/// 把一段或多段 cookie 归一为 `name=value; name2=value2`：按 `;` 切分、
/// 去首尾空白与空段、丢弃没有 `=` 的残段；同名对去重（后者覆盖前者，
/// 位置保留首次出现——与浏览器后写覆盖先写的语义一致）。
fn normalize_pairs(raw: &str) -> String {
    let mut names: Vec<String> = Vec::new();
    let mut pairs: Vec<String> = Vec::new();
    for part in raw.split(';') {
        let part = part.trim();
        if part.is_empty() {
            continue;
        }
        let name = part.split('=').next().unwrap_or("").trim();
        if name.is_empty() {
            continue;
        }
        if let Some(pos) = names.iter().position(|n| n == name) {
            pairs[pos] = part.to_string();
        } else {
            names.push(name.to_string());
            pairs.push(part.to_string());
        }
    }
    pairs.join("; ")
}

// ============================================================
// 浏览器仿真头
// ============================================================

/// Chrome UA（cookie 型站点按桌面 Chrome 请求仿真，版本随主线定期更新）。
const CHROME_UA: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36";

/// 生成 cookie 型 provider 共用的浏览器仿真头：Cookie + Accept /
/// Accept-Language / User-Agent（Chrome）+ Origin + Referer。所有取值均
/// 由调用方传入（Origin/Referer 用所选站点及其页面路径）。
pub(crate) fn chrome_like_headers(
    cookie: &str,
    origin: &str,
    referer: &str,
) -> Vec<(String, String)> {
    vec![
        ("Cookie".to_string(), cookie.to_string()),
        (
            "Accept".to_string(),
            "application/json, text/plain, */*".to_string(),
        ),
        ("Accept-Language".to_string(), "en-US,en;q=0.9".to_string()),
        ("User-Agent".to_string(), CHROME_UA.to_string()),
        ("Origin".to_string(), origin.to_string()),
        ("Referer".to_string(), referer.to_string()),
    ]
}

// ============================================================
// 时间自适应解析（cookie 型 provider 的 reset/expire 字段复用）
// ============================================================

/// epoch 秒/毫秒自适应（纯函数）：>10^12 视为毫秒，否则按秒 ×1000，
/// 统一归一为毫秒时间戳（前端 resetsAt/展示均为 ms 口径，与 minimax 一致）。
pub(crate) fn epoch_to_ms(raw: f64) -> i64 {
    if raw > 1_000_000_000_000.0 {
        raw as i64
    } else {
        (raw * 1000.0) as i64
    }
}

/// 时间字段弹性解析（纯函数）：数字（或纯数字串）按 epoch 秒/毫秒自适应；
/// 字符串依次尝试 ISO-8601 / RFC3339、`yyyy-MM-dd HH:mm:ss`、`yyyy-MM-dd`
///（后两者无时区标记，按 UTC 解释）。全部失败返回 None。
pub(crate) fn parse_time_flexible(v: &serde_json::Value) -> Option<i64> {
    if let Some(n) = crate::provider_quota::parse_flexible_f64(v) {
        return Some(epoch_to_ms(n));
    }
    let s = v.as_str()?.trim();
    if s.is_empty() {
        return None;
    }
    if let Ok(n) = s.parse::<f64>() {
        return Some(epoch_to_ms(n));
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Some(dt.timestamp_millis());
    }
    if let Ok(dt) = chrono::NaiveDateTime::parse_from_str(s, "%Y-%m-%d %H:%M:%S") {
        return Some(dt.and_utc().timestamp_millis());
    }
    if let Ok(d) = chrono::NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let dt = d.and_hms_opt(0, 0, 0)?;
        return Some(dt.and_utc().timestamp_millis());
    }
    None
}

// ============================================================
// 单元测试（纯函数，不联网）
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// 形态 1：裸 cookie 串原样归一（含空白修剪）；前导 `Cookie:` 头名容忍。
    #[test]
    fn bare_cookie_passthrough() {
        assert_eq!(
            normalize_cookie_secret("sessionid=abc; uid=42"),
            "sessionid=abc; uid=42"
        );
        // 前导 Cookie: 头名 + 首尾空白
        assert_eq!(
            normalize_cookie_secret("  Cookie: sessionid=abc; uid=42  "),
            "sessionid=abc; uid=42"
        );
        // 包裹引号（多层）
        assert_eq!(
            normalize_cookie_secret("'\"sessionid=abc\"'"),
            "sessionid=abc"
        );
    }

    /// 形态 2：整段 Copy as cURL（单引号 -H Cookie 头）提取。
    #[test]
    fn curl_single_h_cookie() {
        let cmd = "curl 'https://qoder.com/api/v2/me/usages/big_model_credits' \
             -H 'User-Agent: Mozilla/5.0' \
             -H 'Cookie: sessionid=abc; uid=42' \
             -H 'Accept: application/json'";
        assert_eq!(
            normalize_cookie_secret(cmd),
            "sessionid=abc; uid=42"
        );
    }

    /// 形态 2 扩展：cURL 多条 -H Cookie 头全部提取拼接；双引号同样支持；
    /// 同名 cookie 后写覆盖先写。
    #[test]
    fn curl_multiple_h_cookies_joined() {
        let cmd = "curl 'https://longcat.chat/api/v1/user-current' \\\n \
             -H \"Cookie: sessionid=old\" \\\n \
             -H 'Cookie: sessionid=new; uid=42' \\\n \
             -H 'Referer: https://longcat.chat/platform/usage'";
        assert_eq!(
            normalize_cookie_secret(cmd),
            "sessionid=new; uid=42"
        );
    }

    /// 形态 2 扩展：--cookie / -b 形态与 bash $'...' 引用；非 Cookie 头不混入。
    #[test]
    fn curl_cookie_flag_and_bash_ansi_quoting() {
        assert_eq!(
            normalize_cookie_secret("curl 'https://x' --cookie 'a=1; b=2'"),
            "a=1; b=2"
        );
        assert_eq!(normalize_cookie_secret("curl 'https://x' -b 'a=1'"), "a=1");
        // $'...'（ANSI-C 引用）
        assert_eq!(
            normalize_cookie_secret("curl 'https://x' -H $'Cookie: a=1'"),
            "a=1"
        );
        // 整段 --cookie 误带头名也容忍
        assert_eq!(
            normalize_cookie_secret("curl 'https://x' --cookie 'Cookie: a=1'"),
            "a=1"
        );
    }

    /// 无 Cookie 的 cURL → 空串（调用方报错），不得把整段命令误当 cookie。
    #[test]
    fn curl_without_cookie_returns_empty() {
        let cmd = "curl 'https://qoder.com/api' -H 'User-Agent: Mozilla/5.0' -H 'Accept: */*'";
        assert_eq!(normalize_cookie_secret(cmd), "");
        // 残缺形态（只有 flag 无参数）同样走空串而不是把 flag 当值
        assert_eq!(normalize_cookie_secret("--header -H --cookie"), "");
        // 空输入
        assert_eq!(normalize_cookie_secret("   "), "");
    }

    /// 粘贴内容含多字节字符（中文说明等）不 panic，flag 仍可正常提取。
    #[test]
    fn multibyte_content_is_safe_to_scan() {
        let cmd = "curl 'https://longcat.chat/api' # 下面是登录态（勿泄露）\n -H 'Cookie: a=1; b=2'";
        assert_eq!(normalize_cookie_secret(cmd), "a=1; b=2");
        // 中文紧贴 flag 前沿（中间无空白）时不构成 flag 起点，按裸串处理
        // （该输入同样会走字节扫描路径，验证多字节字符不 panic）
        let bare = "会话a=1; b=2";
        assert_eq!(normalize_cookie_secret(bare), "会话a=1; b=2");
    }

    /// chrome_like_headers：六个仿真头齐全、值正确（供 provider 复用的契约）。
    #[test]
    fn chrome_headers_shape() {
        let headers = chrome_like_headers("a=1", "https://qoder.com", "https://qoder.com/account/usage");
        assert_eq!(headers.len(), 6);
        let get = |name: &str| {
            headers
                .iter()
                .find(|(k, _)| k == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!(get("Cookie"), Some("a=1"));
        assert_eq!(get("Accept"), Some("application/json, text/plain, */*"));
        assert_eq!(get("Accept-Language"), Some("en-US,en;q=0.9"));
        assert_eq!(
            get("User-Agent"),
            Some("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/143.0.0.0 Safari/537.36")
        );
        assert_eq!(get("Origin"), Some("https://qoder.com"));
        assert_eq!(get("Referer"), Some("https://qoder.com/account/usage"));
    }

    /// 时间弹性解析：epoch 秒/毫秒（数字与纯数字串）、ISO-8601、
    /// `yyyy-MM-dd HH:mm:ss`、`yyyy-MM-dd`、脏值 None。
    #[test]
    fn time_flexible_parses_all_shapes() {
        assert_eq!(parse_time_flexible(&serde_json::json!(1_730_000_000)), Some(1_730_000_000_000));
        assert_eq!(
            parse_time_flexible(&serde_json::json!(1_730_000_000_000i64)),
            Some(1_730_000_000_000)
        );
        assert_eq!(
            parse_time_flexible(&serde_json::json!("1730000000")),
            Some(1_730_000_000_000)
        );
        assert_eq!(
            parse_time_flexible(&serde_json::json!("2030-10-27T05:06:07Z")),
            Some(1_919_307_967_000)
        );
        assert_eq!(
            parse_time_flexible(&serde_json::json!("2030-10-27 05:06:07")),
            Some(1_919_307_967_000)
        );
        assert_eq!(
            parse_time_flexible(&serde_json::json!("2030-10-27")),
            Some(1_919_289_600_000)
        );
        assert_eq!(parse_time_flexible(&serde_json::json!("not-a-time")), None);
        assert_eq!(parse_time_flexible(&serde_json::json!(null)), None);
    }
}

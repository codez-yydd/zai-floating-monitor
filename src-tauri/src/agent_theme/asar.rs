//! @electron/asar CLI 封装：extract / pack / extract-file。
//!
//! 全部通过 `npx -y @electron/asar` 调用（首次运行自动下载包，之后走缓存）。
//! macOS 下 GUI 应用（Finder/Dock 启动）不继承终端 PATH，npx 定位依次尝试：
//! PATH → /opt/homebrew/bin → /usr/local/bin → ~/.volta/bin →
//! nvm 版本目录扫描（数值降序取最新）→ 登录 shell 兜底探测。
//! 命中绝对路径后会把 npx 所在目录注入子进程 PATH 前部：npx 脚本 shebang 为
//! `#!/usr/bin/env node`，子进程 PATH 里没有 node 时执行照样失败。
//! Windows 统一用 npx.cmd 并加 CREATE_NO_WINDOW（参考 accounts.rs run_hidden
//! 的防黑窗处理），PATH 注入对 npx.cmd 内部查找 node 同样生效。

use std::path::{Path, PathBuf};
use std::process::Command;
// Stdio 仅登录 shell 兜底探测使用（Windows 无该探测）
#[cfg(not(windows))]
use std::process::Stdio;
use std::sync::OnceLock;

/// npx 可执行文件定位结果缓存（探测要跑子进程，避免重复开销）
static NPX: OnceLock<Option<PathBuf>> = OnceLock::new();

/// Windows 隐藏控制台窗口标志（CREATE_NO_WINDOW）
#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// 把目录注入子进程 PATH 最前部（Windows 下分隔符为 `;`）。
/// npx 所在目录通常也存放同版本 node，注入后 `#!/usr/bin/env node` 才能命中。
fn prepend_path(cmd: &mut Command, dir: &Path) {
    let sep = if cfg!(windows) { ";" } else { ":" };
    let orig = std::env::var("PATH").unwrap_or_default();
    cmd.env("PATH", format!("{}{sep}{}", dir.to_string_lossy(), orig));
}

/// 探测单个 npx 候选是否可用。
/// 候选为绝对路径时，把其所在目录注入子进程 PATH 前部：
/// npx 脚本 shebang 为 `#!/usr/bin/env node`，GUI 启动的进程 PATH 里往往
/// 没有 node（典型如 Homebrew 场景），不注入会导致探测误判失败。
fn probe(candidate: &Path) -> bool {
    let mut cmd = Command::new(candidate);
    if candidate.is_absolute() {
        if let Some(dir) = candidate.parent() {
            prepend_path(&mut cmd, dir);
        }
    }
    cmd.arg("--version");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    cmd.output().map(|o| o.status.success()).unwrap_or(false)
}

/// 扫描 nvm 的版本目录（如 `~/.nvm/versions/node`），按版本号数值降序
/// 取第一个含 `bin/npx` 的路径。
/// 版本比较解析为数字元组而非字符串排序：字符串降序会把 v9 排在 v10 前面。
fn scan_nvm_npx(nvm_home: &Path) -> Option<PathBuf> {
    let mut versions: Vec<(Vec<u32>, PathBuf)> = std::fs::read_dir(nvm_home)
        .ok()?
        .filter_map(|e| e.ok())
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            // 目录名形如 v20.19.5，非版本命名的目录直接忽略
            let name = e.file_name().to_string_lossy().into_owned();
            let num = name.strip_prefix('v')?;
            let parts: Option<Vec<u32>> = num.split('.').map(|p| p.parse().ok()).collect();
            let parts = parts?;
            Some((parts, e.path()))
        })
        .collect();
    // 数值元组降序：Vec<u32> 逐元素比较，[2,0] < [2,0,1] 符合语义版本语义
    versions.sort_by(|a, b| b.0.cmp(&a.0));
    versions
        .into_iter()
        .map(|(_, dir)| dir.join("bin/npx"))
        .find(|p| p.is_file())
}

/// 登录 shell 兜底探测：GUI 进程不继承终端 PATH，而 nvm/fnm 等版本管理器
/// 依赖 rc 文件（~/.zshrc 等）初始化 PATH，这里用交互式 shell 执行
/// `command -v npx` 获取绝对路径。依次尝试 SHELL 环境变量、/bin/zsh、/bin/bash。
#[cfg(not(windows))]
fn probe_via_shell() -> Option<PathBuf> {
    let mut shells: Vec<String> = Vec::new();
    if let Ok(s) = std::env::var("SHELL") {
        if !s.trim().is_empty() {
            shells.push(s);
        }
    }
    shells.push("/bin/zsh".to_string());
    shells.push("/bin/bash".to_string());

    for shell in shells {
        if !Path::new(&shell).is_file() {
            continue;
        }
        let mut cmd = Command::new(&shell);
        // -i 交互模式让 rc 文件里的 nvm 初始化生效；stdin 关闭避免交互等待，
        // stderr 丢弃（-i 模式可能输出 job control 等警告），stdout 留管道读结果
        cmd.arg("-i").arg("-c").arg("command -v npx");
        cmd.stdin(Stdio::null()).stderr(Stdio::null());
        let output = match cmd.output() {
            Ok(o) => o,
            Err(_) => continue,
        };
        if !output.status.success() {
            continue;
        }
        // rc 文件可能有 echo 干扰 stdout，`command -v npx` 的结果通常在最后，
        // 因此从后往前找第一个真实存在的路径
        let found = String::from_utf8_lossy(&output.stdout)
            .lines()
            .rev()
            .map(str::trim)
            .filter(|l| !l.is_empty())
            .map(PathBuf::from)
            .find(|p| p.is_file());
        if let Some(p) = found {
            return Some(p);
        }
    }
    None
}

/// Windows 无 POSIX 登录 shell 兜底场景，直接不做该探测。
#[cfg(windows)]
fn probe_via_shell() -> Option<PathBuf> {
    None
}

/// 定位可用的 npx。找不到返回 None。结果经 OnceLock 缓存，进程内只探测一次。
fn resolve_npx() -> Option<PathBuf> {
    NPX.get_or_init(|| {
        if cfg!(windows) {
            // Windows：npx 实为 npx.cmd 批处理，直接调 npx 会打不开
            if probe(Path::new("npx.cmd")) {
                return Some(PathBuf::from("npx.cmd"));
            }
            return None;
        }
        if probe(Path::new("npx")) {
            return Some(PathBuf::from("npx"));
        }
        // GUI 进程 PATH 可能缺 Homebrew：按常见安装位置兜底
        for candidate in ["/opt/homebrew/bin/npx", "/usr/local/bin/npx"] {
            if probe(Path::new(candidate)) {
                return Some(PathBuf::from(candidate));
            }
        }
        // volta 版本管理器固定安装位置
        if let Some(home) = std::env::var_os("HOME") {
            let volta = PathBuf::from(&home).join(".volta/bin/npx");
            if probe(&volta) {
                return Some(volta);
            }
        }
        // nvm：扫描版本目录，取数值最新的含 npx 版本
        if let Some(home) = std::env::var_os("HOME") {
            let nvm_versions = PathBuf::from(&home).join(".nvm/versions/node");
            if let Some(npx) = scan_nvm_npx(&nvm_versions) {
                if probe(&npx) {
                    return Some(npx);
                }
            }
        }
        // 最终兜底：借登录 shell 的 rc 初始化定位 npx（fnm/n 等其他版本管理器也覆盖）
        probe_via_shell()
    })
    .clone()
}

/// node/npx 预检：注入流程（asar 解包/重打包）的前置条件。
pub fn node_available() -> bool {
    resolve_npx().is_some()
}

/// 执行一条 @electron/asar 子命令，成功返回 stdout。
fn run_asar(args: &[&str]) -> Result<String, String> {
    run_asar_in(None, args)
}

/// 执行一条 @electron/asar 子命令，可选指定子进程工作目录
/// （extract-file 会把文件写到 cwd，需要指向受控临时目录）。
fn run_asar_in(cwd: Option<&Path>, args: &[&str]) -> Result<String, String> {
    let npx = resolve_npx().ok_or("未检测到 Node.js/npx，请先安装 Node.js（https://nodejs.org）")?;
    let mut cmd = Command::new(&npx);
    // npx 为绝对路径时，把其所在目录注入子进程 PATH 前部：
    // npx 脚本 shebang 为 `#!/usr/bin/env node`（Windows 下 npx.cmd 内部同样
    // 依赖 PATH 找 node），GUI 启动的进程 PATH 里没有 node 时执行会失败
    if npx.is_absolute() {
        if let Some(dir) = npx.parent() {
            prepend_path(&mut cmd, dir);
        }
    }
    cmd.arg("-y").arg("@electron/asar").args(args);
    if let Some(dir) = cwd {
        cmd.current_dir(dir);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }
    let output = cmd
        .output()
        .map_err(|e| format!("主题处理工具调用失败（{e}），请确认 Node.js 安装正常"))?;
    if !output.status.success() {
        let stderr_raw = String::from_utf8_lossy(&output.stderr);
        let stdout_raw = String::from_utf8_lossy(&output.stdout);
        let sub = args.first().copied().unwrap_or("");
        // 过滤 notice 提炼详情；过滤后为空时回退未过滤原文，详情保证非空
        let detail = extract_failure_detail(&stderr_raw, &stdout_raw);
        // npx/electron 的原始输出仅进日志，不直接透传给用户可见层
        println!("[zbar] asar {sub} 原始输出：{detail}");
        // 用户可见错误保留简短中文原因，原始输出截断到 200 字符内附后辅助排查
        let short: String = detail.chars().take(200).collect();
        return Err(format!("主题资源处理失败（asar {sub}）：{short}"));
    }
    Ok(strip_npm_noise(&String::from_utf8_lossy(&output.stdout)))
}

/// 过滤 npm 12 起 npx 运行时混入 stdout/stderr 的 `npm notice run ...`
/// 提示行：不过滤会污染 list 的路径清单解析、挤占错误详情的 200 字符
/// 截断窗口（真实错误被 notice 行淹没，用户只看到 npm 输出无法排查）。
/// `npm error` 行不在此过滤：npx 层失败（如网络断开导致下载
/// @electron/asar 遇 E404/ENOTFOUND）时 stderr 往往只剩 error 行，
/// 其中错误码、域名是仅有的诊断信息，剔除会让错误详情变空、无法排查。
/// 纯函数，便于单测。
fn strip_npm_noise(text: &str) -> String {
    text.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.starts_with("npm notice")
        })
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_string()
}

/// 失败详情提炼：优先取过滤 notice 后的 stderr，为空再取 stdout；
/// 两者过滤后均为空时回退未过滤原文（trim 后）——防御层，保证任何
/// 情况下错误详情不为空（如输出全为 notice 行时仍能看到 npm 原始输出）。
/// 纯函数，便于单测。
fn extract_failure_detail(stderr_raw: &str, stdout_raw: &str) -> String {
    let filtered_stderr = strip_npm_noise(stderr_raw);
    let detail = if filtered_stderr.is_empty() {
        strip_npm_noise(stdout_raw)
    } else {
        filtered_stderr
    };
    if !detail.is_empty() {
        return detail;
    }
    // 回退未过滤原文：保留 notice 等行里仅存的原始诊断信息
    if !stderr_raw.trim().is_empty() {
        stderr_raw.trim().to_string()
    } else {
        stdout_raw.trim().to_string()
    }
}

/// 解包 asar 到目标目录（extract）。
pub fn asar_extract(src: &Path, dest: &Path) -> Result<(), String> {
    if !src.is_file() {
        return Err(format!("asar 文件不存在：{}", src.display()));
    }
    run_asar(&["extract", &src.to_string_lossy(), &dest.to_string_lossy()])
        .map_err(|e| format!("准备主题资源失败：{e}"))?;
    Ok(())
}

/// 打包目录为 asar（pack），按 `unpack_glob` 把匹配文件拆到 `<dest>.unpacked`，
/// Electron 运行时按 asar 外部路径加载这些文件。
///
/// `unpack_glob` 由调用方在运行时动态构造（见 mod.rs build_unpack_glob）：
/// 官方 unpacked 目录现状的全部文件相对路径 + `**/*.node` 兜底——
/// 新 asar 的外置清单动态等于官方目录现状，官方目录天然满足新 asar
/// 的引用（替换脚本因此不再同步/触碰官方 unpacked 目录）。
pub fn asar_pack_with_unpack(dir: &Path, dest: &Path, unpack_glob: &str) -> Result<(), String> {
    if !dir.is_dir() {
        return Err(format!("待打包目录不存在：{}", dir.display()));
    }
    let glob = unpack_glob.trim();
    if glob.is_empty() {
        return Err("外置清单 glob 不能为空".to_string());
    }
    run_asar(&[
        "pack",
        &dir.to_string_lossy(),
        &dest.to_string_lossy(),
        "--unpack",
        glob,
    ])
    .map_err(|e| format!("应用主题失败：{e}"))?;
    Ok(())
}

/// 抽取 asar 内单个文件内容（extract-file，用于注入标记抽检）。
///
/// 注意：@electron/asar CLI 的 extract-file 实际行为是把文件写到
/// **当前工作目录**（basename），并非输出到 stdout。因此这里以临时目录
/// 作为子进程 cwd，执行后读回文件内容再清理。
pub fn asar_extract_file_to_stdout(asar: &Path, inner_path: &str) -> Result<String, String> {
    if !asar.is_file() {
        return Err(format!("asar 文件不存在：{}", asar.display()));
    }
    // basename：CLI 落盘文件名（out/renderer/index.html → index.html）
    let base = inner_path
        .rsplit('/')
        .next()
        .filter(|s| !s.is_empty())
        .ok_or_else(|| format!("asar 内路径非法：{inner_path}"))?;

    let work_dir = std::env::temp_dir().join(format!(
        "zbar-asar-probe-{}-{}",
        std::process::id(),
        chrono::Utc::now().timestamp_nanos_opt().unwrap_or(0)
    ));
    std::fs::create_dir_all(&work_dir)
        .map_err(|e| format!("创建抽检临时目录失败: {e}"))?;

    let result = (|| -> Result<String, String> {
        // @electron/asar v4 起在 Windows 上按 Windows 路径语义索引包内条目，
        // 正斜杠内路径报 "was not found in this archive"（v3 两平台均只认
        // 正斜杠）。故 Windows 先按 v4 的反斜杠形式请求，失败回退正斜杠
        // 原样重试一次，兼容 npx 缓存中仍为 v3 的机器；回退仅发生在失败
        // 路径（失败本就意味着安装即将中止），正常情况零额外开销。POSIX
        // 平台两版本路径语义一致，不做转换。
        #[cfg(windows)]
        let attempts: Vec<String> = vec![inner_path.replace('/', "\\"), inner_path.to_string()];
        #[cfg(not(windows))]
        let attempts: Vec<String> = vec![inner_path.to_string()];

        let mut last_err = String::new();
        for attempt in &attempts {
            match run_asar_in(Some(&work_dir), &["extract-file", &asar.to_string_lossy(), attempt])
            {
                // extract-file 的 stdout 无内容价值，文件内容从落盘读回
                Ok(_) => {
                    let content_path = work_dir.join(base);
                    return std::fs::read_to_string(&content_path).map_err(|e| {
                        format!("读取抽检文件失败（{}）: {e}", content_path.display())
                    });
                }
                Err(e) => last_err = e,
            }
        }
        Err(format!("抽取 asar 内文件失败（{inner_path}）：{last_err}"))
    })();

    // 无论成败都清理临时目录
    let _ = std::fs::remove_dir_all(&work_dir);
    result
}

/// list 输出行归一化为 `/` 分隔、无前导分隔符的 rel 路径（纯函数，便于单测）。
/// @electron/asar v4 起在 Windows 上输出 `\out\renderer\index.html` 风格
/// （v3 两平台均为 `/out/renderer/index.html`），不归一化会导致清单比对
/// 靠两侧格式对称侥幸通过，语义上已是错误行为。
fn normalize_list_line(line: &str) -> String {
    line.trim()
        .trim_start_matches(['/', '\\'])
        .replace('\\', "/")
}

/// 列出 asar 内全部文件路径（list，安装校验的文件清单比对用）。
/// 输出统一为 `/` 分隔、无前导分隔符的 rel 路径（normalize_list_line），
/// 调用方与平台、@electron/asar 版本无关。
pub fn asar_list(asar: &Path) -> Result<Vec<String>, String> {
    let out = run_asar(&["list", &asar.to_string_lossy()])
        .map_err(|e| format!("列出 asar 内容失败：{e}"))?;
    Ok(out
        .lines()
        .map(normalize_list_line)
        .filter(|l| !l.is_empty())
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// npm 12 噪音过滤（strip_npm_noise）：notice 行剔除，npm error 行
    /// 作为诊断信息保留，asar 真实输出与空行以外的内容保留。
    #[test]
    fn npm噪音过滤_剔除notice保留error与真实输出() {
        // v4 Windows 真机错误形态：notice 行 + 真实错误行混杂
        let mixed = "npm notice run npx\nnpm notice run asar extract-file a.asar out/renderer/index.html\nfile:///C:/npm-cache/_npx/\nError: \"x\" was not found in this archive";
        let stripped = strip_npm_noise(mixed);
        assert!(!stripped.contains("npm notice"), "notice 行应被剔除：{stripped}");
        assert!(stripped.contains("was not found"), "真实错误应保留：{stripped}");
        assert!(stripped.contains("file:///"), "非 notice 行应保留：{stripped}");
        // 全 notice → 空（走 stderr 优先逻辑时不会再被 notice 占位）
        assert_eq!(strip_npm_noise("npm notice run npx\nnpm notice run asar list a"), "");
        // 无噪音输出原样（仅 trim）
        assert_eq!(strip_npm_noise("v4.3.0\n"), "v4.3.0");
        // npm error 行是诊断信息（错误码等），保留不过滤
        assert_eq!(
            strip_npm_noise("npm error code E404\nreal"),
            "npm error code E404\nreal"
        );
    }

    /// 失败详情提炼（extract_failure_detail）：npx 层失败（如网络断开
    /// 导致下载 @electron/asar 遇 E404/ENOTFOUND）时 stderr 全为
    /// npm error 行，这些诊断行（错误码、域名等）应原样进入详情。
    #[test]
    fn 失败详情_stderr全error行时保留诊断信息() {
        let stderr_raw = "npm error code ENOTFOUND\nnpm error syscall getaddrinfo\nnpm error registry.npmjs.org";
        let detail = extract_failure_detail(stderr_raw, "");
        assert!(detail.contains("ENOTFOUND"), "错误码应保留：{detail}");
        assert!(detail.contains("registry.npmjs.org"), "域名应保留：{detail}");
        // stderr 过滤后为空时取 stdout 的过滤结果
        assert_eq!(extract_failure_detail("", "npm notice run npx\nError: ENOENT"), "Error: ENOENT");
    }

    /// 失败详情提炼（extract_failure_detail）防御层：过滤后为空
    /// （如输出全为 notice 行）时回退未过滤原文，保证详情不为空。
    #[test]
    fn 失败详情_过滤后为空回退未过滤原文() {
        // 全 notice 输出：过滤后为空，回退 stderr 原文（trim 后）
        let stderr_raw = "npm notice run npx\nnpm notice run asar list a\n";
        assert_eq!(
            extract_failure_detail(stderr_raw, "npm notice from stdout"),
            stderr_raw.trim(),
            "应回退未过滤的 stderr 原文"
        );
        // stderr 原文为空白时回退 stdout 原文
        assert_eq!(
            extract_failure_detail("  \n", "npm notice from stdout\n"),
            "npm notice from stdout",
            "stderr 原文空白时应回退 stdout 原文"
        );
    }

    /// list 行归一化（normalize_list_line）：v4 Windows 反斜杠与 v3 POSIX
    /// 风格统一为 `/` 分隔、无前导分隔符的 rel 路径。
    #[test]
    fn list行归一化_反斜杠与前导分隔符统一() {
        // v4 Windows 实测形态
        assert_eq!(normalize_list_line(r"\out\renderer\index.html"), "out/renderer/index.html");
        // v3 POSIX 形态
        assert_eq!(normalize_list_line("/out/renderer/index.html"), "out/renderer/index.html");
        // 已归一化输入幂等
        assert_eq!(normalize_list_line("out/main.js"), "out/main.js");
        // 目录条目（无扩展名文件路径同样处理）
        assert_eq!(normalize_list_line(r"\out\renderer"), "out/renderer");
        // 纯空白行归一化为空（由 asar_list 的 filter 剔除）
        assert_eq!(normalize_list_line("   "), "");
    }

    #[test]
    fn node_预检_结果一致() {
        // 本机装有 node/npx 时应可用；未装时也应稳定返回 false 而非 panic
        let a = node_available();
        let b = node_available();
        assert_eq!(a, b, "探测结果应被缓存且稳定");
    }

    /// nvm 扫描：版本目录按数值降序选中（v9 与 v10 字符串排序会排错）。
    #[test]
    fn nvm扫描_按版本号数值降序选中() {
        let root = std::env::temp_dir().join(format!("zbar-nvm-scan-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        for v in ["v9.0.0", "v10.1.0"] {
            let dir = root.join(v);
            std::fs::create_dir_all(dir.join("bin")).unwrap();
            std::fs::write(dir.join("bin/npx"), "#!/usr/bin/env node").unwrap();
        }
        // 干扰目录：非版本命名，应被忽略
        std::fs::create_dir_all(root.join("alias")).unwrap();

        let picked = scan_nvm_npx(&root).expect("应找到 npx");
        assert!(
            picked.ends_with("v10.1.0/bin/npx"),
            "应选数值更大的 v10.1.0，实际选中：{}",
            picked.display()
        );

        let _ = std::fs::remove_dir_all(&root);
    }

    /// nvm 扫描：空目录 / 目录不存在应稳定返回 None。
    #[test]
    fn nvm扫描_空目录返回空() {
        let root = std::env::temp_dir().join(format!("zbar-nvm-empty-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        assert_eq!(scan_nvm_npx(&root), None, "空目录应返回 None");
        let missing = root.join("not-exist");
        assert_eq!(scan_nvm_npx(&missing), None, "不存在的目录应返回 None");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// 模拟 GUI 启动环境（子进程清空全部环境变量、仅保留 HOME/SHELL，
    /// 不继承 PATH），验证登录 shell 兜底能通过 rc 文件初始化定位 npx。
    /// 仅当本机存在 ~/.nvm（nvm 安装场景）时强断言，其余环境跳过。
    /// macOS/Linux 专属：Windows 无 POSIX 登录 shell 兜底（probe_via_shell
    /// 恒为 None），测试依赖的 /bin/zsh 亦不存在。
    #[test]
    #[cfg(not(windows))]
    fn 清空path模拟gui_登录shell兜底可定位npx() {
        let nvm_versions = std::env::var_os("HOME")
            .map(|h| PathBuf::from(h).join(".nvm/versions/node"))
            .filter(|p| p.is_dir());

        let mut cmd = Command::new("/bin/zsh");
        cmd.arg("-i").arg("-c").arg("command -v npx");
        cmd.env_clear();
        if let Some(home) = std::env::var_os("HOME") {
            cmd.env("HOME", home);
        }
        if let Some(shell) = std::env::var_os("SHELL") {
            cmd.env("SHELL", shell);
        }
        cmd.stdin(Stdio::null()).stderr(Stdio::null());
        let output = cmd.output().expect("应能启动 zsh 子进程");

        let found = String::from_utf8_lossy(&output.stdout)
            .lines()
            .rev()
            .map(str::trim)
            .find(|l| !l.is_empty() && Path::new(l).is_file())
            .map(PathBuf::from);

        if nvm_versions.is_some() {
            assert!(
                found.is_some(),
                "清空 PATH 的 GUI 环境下应通过登录 shell 找到 nvm 的 npx"
            );
            assert!(found.unwrap().is_absolute(), "兜底结果应为绝对路径");
        }
    }

    /// 端到端闭环：自造小 asar → pack → extract-file 抽检读回内容。
    /// 仅使用临时目录，不触碰任何真实应用。无 node 环境时跳过。
    #[test]
    fn 抽检_从自造_asar_读回注入标记() {
        if !node_available() {
            return;
        }
        let dir = std::env::temp_dir().join(format!("zbar-asar-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let src = dir.join("src");
        std::fs::create_dir_all(src.join("out/renderer")).unwrap();
        std::fs::write(
            src.join("out/renderer/index.html"),
            "<html><head></head><body><!--ZBAR-THEME-BEGIN--></body></html>",
        )
        .unwrap();

        asar_pack_with_unpack(&src, &dir.join("app.asar"), "**/*.node")
            .expect("打包自造 asar 应成功");
        let html = asar_extract_file_to_stdout(&dir.join("app.asar"), "out/renderer/index.html")
            .expect("抽检应读回 index.html 内容");
        assert!(html.contains("ZBAR-THEME-BEGIN"), "抽检内容应含注入标记");

        // 不存在的内路径应报中文错误而非 panic
        let err = asar_extract_file_to_stdout(&dir.join("app.asar"), "out/renderer/none.html");
        assert!(err.is_err());

        // 空 glob 防御：直接报中文错误，不落到 npx 子进程
        assert!(asar_pack_with_unpack(&src, &dir.join("bad.asar"), "  ").is_err());

        let _ = std::fs::remove_dir_all(&dir);
    }
}

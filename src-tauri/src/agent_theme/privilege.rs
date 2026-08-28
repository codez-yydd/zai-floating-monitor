//! 提权兜底执行（两平台共用接口：run_as_admin / is_admin_cancelled）。
//!
//! ## macOS：osascript "with administrator privileges"
//!
//! 历史上这是替换 /Applications 下应用文件的主路径；真机实测发现它在
//! macOS「应用管理」(App Management TCC) 下是死结：提权 shell 中 cp 等
//! 写入操作的权限责任方无法归到 ZBar，会被静默拒绝（Operation not
//! permitted）且系统设置里找不到可授权项。现仅作为「应用真的无写权限」
//! （如 root 安装的应用）时的兜底路径；主路径是 ZBar 进程直接执行脚本
//! （TCC 责任归 ZBar，首次弹系统标准允许框），见 mod.rs 的
//! execute_replace_script。脚本内路径一律用 sh_quote 做 POSIX 单引号转义。
//!
//! ## Windows：PowerShell UAC（Start-Process -Verb RunAs）
//!
//! 无写权限（如装在 Program Files 下的应用）时的兜底路径。为规避多层
//! 引号转义与中文路径代码页问题，采用「临时 .cmd 脚本文件」方案：
//! 把要执行的操作（copy/move 序列，由 mod.rs 的 build_admin_script 构造）
//! 写入 %TEMP% 下全 ASCII 文件名的脚本，PowerShell 以管理员身份运行该
//! 脚本并等待退出（-Wait -PassThru），退出码经 powershell 进程透传；
//! 用户在 UAC 弹窗点「否」时识别为取消（退出码 1223，is_admin_cancelled
//! 兼容）。脚本内容按 UTF-8 无 BOM 写入，首行 chcp 65001 保证中文路径
//! 被正确解析。

/// POSIX shell 单引号转义：'…' 内出现单引号时拆分为 '\'' 三段拼接。
/// 所有拼进提权脚本的路径/字符串都必须先经过本函数。
/// 接受 &str / &Path / &PathBuf（路径自动按系统编码转字符串）。
/// （Windows 无 shell 脚本路径，仅 macOS/Linux 构造脚本时使用）
#[cfg_attr(windows, allow(dead_code))]
pub fn sh_quote<S: AsRef<std::ffi::OsStr>>(s: S) -> String {
    let s = s.as_ref().to_string_lossy();
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// AppleScript 字符串字面量转义（反斜杠与双引号）
#[cfg(target_os = "macos")]
fn escape_applescript(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

/// 以管理员权限执行一段 shell 脚本（macOS 弹密码框，prompt 为授权提示文案）。
/// - 脚本须为单行（多命令用 && / ; 连接，不能含裸换行）
/// - 路径参数请先用 sh_quote 转义
/// - 成功返回 stdout
#[cfg(target_os = "macos")]
pub fn run_as_admin(script: &str, prompt: &str) -> Result<String, String> {
    let apple = format!(
        "do shell script \"{}\" with administrator privileges with prompt \"{}\"",
        escape_applescript(script),
        escape_applescript(prompt)
    );
    let output = std::process::Command::new("osascript")
        .args(["-e", &apple])
        .output()
        .map_err(|e| format!("调用 osascript 失败：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        // 用户点"取消"（-128）时给出更友好的中文提示
        if stderr.contains("-128") || stderr.contains("User canceled") {
            return Err("已取消管理员授权".into());
        }
        return Err(format!("管理员授权执行失败：{stderr}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// PowerShell 单引号字面量转义（' → ''，中文路径原样保留）。
/// 脚本路径经 CreateProcessW 命令行（UTF-16）传入 PowerShell，无代码页
/// 问题，唯一需要处理的是路径自身含单引号的罕见场景。
#[cfg(windows)]
fn escape_ps_single(s: &str) -> String {
    s.replace('\'', "''")
}

/// 以管理员权限执行一段 cmd 批处理脚本（Windows 弹 UAC 确认框）。
///
/// 实现链路（见模块头「Windows」节）：
/// 1. 把脚本写入 %TEMP%\zbar-elevate-<纳秒时间戳>.cmd（文件名全 ASCII，
///    内容 UTF-8 无 BOM + 首行 chcp 65001，中文路径可正确解析；换行统一
///    转为 CRLF，cmd 批处理最稳的分隔方式）；
/// 2. `powershell -NoProfile -Command "Start-Process cmd -Verb RunAs -Wait
///    -PassThru"` 以管理员运行该脚本并等待退出，`exit $p.ExitCode` 把
///    cmd 退出码透传为 powershell 退出码（全程 CREATE_NO_WINDOW 防黑窗，
///    UAC 弹窗本身由系统展示）；
/// 3. 无论成败删除临时脚本后归档退出码：0 成功；1223 为用户在 UAC 弹窗
///    点「否」（ERROR_CANCELLED，PowerShell catch 分支显式转换为 1223），
///    转为与 macOS 相同的取消文案（is_admin_cancelled 兼容）；其余报中文
///    错误并附 stderr。
///
/// `prompt` 在 Windows 上无法自定义（UAC 弹窗文案由系统按目标程序生成），
/// 仅作签名兼容保留。
#[cfg(windows)]
pub fn run_as_admin(script: &str, _prompt: &str) -> Result<String, String> {
    use std::fs;

    // 1. 临时 .cmd：文件名全 ASCII。TEMP 路径可能含中文用户名，但该路径
    //    经 UTF-16 命令行传给 PowerShell 无损；cmd 解析脚本内容中的中文
    //    路径则依赖 chcp 65001 + UTF-8 无 BOM 的组合（脚本首行已含）
    let ts = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let script_path = std::env::temp_dir().join(format!("zbar-elevate-{ts}.cmd"));
    let crlf = script.replace('\n', "\r\n");
    fs::write(&script_path, crlf.as_bytes())
        .map_err(|e| format!("写入提权临时脚本失败（{}）: {e}", script_path.display()))?;

    // 2. UAC 提权执行：单引号字面量嵌脚本路径；catch 分支把取消异常
    //    （HResult -2147023673 = 0x800704CB = HRESULT_FROM_WIN32(1223)）
    //    归一为退出码 1223，其余异常归一为 1
    let path_lit = escape_ps_single(&script_path.to_string_lossy());
    let ps = format!(
        "try {{ $p = Start-Process -FilePath 'cmd.exe' \
         -ArgumentList '/c \"{path_lit}\"' -Verb RunAs -Wait -PassThru; \
         exit $p.ExitCode }} \
         catch {{ if ($_.Exception.HResult -eq -2147023673 \
         -or \"$($_.Exception.Message)\" -like '*canceled*') {{ exit 1223 }} \
         else {{ exit 1 }} }}"
    );
    let output = crate::accounts::run_hidden("powershell", &["-NoProfile", "-Command", &ps])
        .ok_or_else(|| "调用 PowerShell 失败".to_string());

    // 3. 无论成败都删除临时脚本
    let _ = fs::remove_file(&script_path);
    let output = output?;

    let code = output.status.code().unwrap_or(-1);
    match code {
        0 => Ok(String::from_utf8_lossy(&output.stdout).to_string()),
        // UAC 取消：与 macOS 相同的取消文案（is_admin_cancelled 兼容）
        1223 => Err("已取消管理员授权".into()),
        _ => {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            Err(if stderr.is_empty() {
                format!("管理员授权执行失败（退出码 {code}）")
            } else {
                format!("管理员授权执行失败：{stderr}")
            })
        }
    }
}

/// 非 macOS/Windows：当前版本仅支持 macOS / Windows 注入流程
#[cfg(not(any(target_os = "macos", windows)))]
pub fn run_as_admin(_script: &str, _prompt: &str) -> Result<String, String> {
    Err("当前平台暂不支持提权操作（仅 macOS / Windows）".into())
}

/// 判断错误是否为"用户取消管理员授权"（osascript -128 取消）。
/// 此类错误代表用户主动放弃授权：调用方应直接中止并原样上抛，
/// 不得重试、不得触发自动还原——否则会再次弹出授权窗，
/// 用户被迫连点多次取消。识别两个特征文案：本模块转换后的中文提示，
/// 以及 osascript 原始 stderr 中的 "User canceled"（防御文案变化）。
pub fn is_admin_cancelled(err: &str) -> bool {
    err.contains("已取消管理员授权") || err.contains("User canceled")
}

/// 「应用管理」(App Management) 拦截时的用户指引文案（含系统设置路径）。
/// 直写路径（ZBar 进程直接执行脚本）与提权兜底路径共用：首次替换会弹
/// 系统标准对话框，允许后永久生效；曾拒绝过的应用会出现在「应用管理」
/// 设置列表里，开关打开即可恢复授权（可发现、可管理）。
/// 链接为 Apple 官方帮助文档《在 Mac 上控制对 App 的访问》。
const OPERATION_NOT_PERMITTED_HINT: &str = "macOS「应用管理」权限拦截：首次替换会弹出「ZBar 想要修改 ZCode」的系统对话框，请点「允许」；若此前点过「不允许」，请打开 系统设置 → 隐私与安全性 → 应用管理，开启列表中 ZBar 的开关后重试（开发模式运行时请添加并允许“终端”）。参考：https://support.apple.com/guide/mac-help/mchl7b2b1e0c/mac";

/// 判断错误是否为 macOS「应用管理」(App Management) 权限拦截特征。
/// macOS 对 /Applications 下其它应用 bundle 的写入可能被 App Management
/// （TCC 机制）拦截，cp / rsync 等进程报 "Operation not permitted"（EPERM）。
/// 直写路径下这通常意味着用户在首次弹窗中点了「不允许」（或曾拒绝过）；
/// 此类错误代表系统权限配置问题，重试与自动还原同样会被拦截：调用方应
/// 与 is_admin_cancelled 同样处理——直接中止上抛（经 clarify_admin_error
/// 转为设置指引），不得重试、不得触发自动还原，否则会连环弹出授权窗。
/// 注意区分 EACCES：普通权限不足报 "Permission denied"，不属于本特征。
pub fn is_operation_not_permitted(err: &str) -> bool {
    err.contains("Operation not permitted")
}

/// 「应用管理」拦截指引文案的独立构造：用于探针阶段即被拦（无原始
/// stderr 可附）的场景；clarify_admin_error 亦复用本文案。
pub fn operation_not_permitted_hint() -> String {
    OPERATION_NOT_PERMITTED_HINT.to_string()
}

/// 替换/还原执行失败的统一转换：命中「应用管理」拦截特征时，替换为
/// 带系统设置指引的引导文案（原始错误附后便于排查）；其余错误原样返回
/// （含用户取消类，由调用方按 is_admin_cancelled 处理）。
pub fn clarify_admin_error(err: String) -> String {
    if is_operation_not_permitted(&err) {
        format!("{OPERATION_NOT_PERMITTED_HINT}\n（原始错误：{err}）")
    } else {
        err
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sh_quote_转义() {
        // 普通路径原样包裹
        assert_eq!(sh_quote("/Applications/ZCode.app"), "'/Applications/ZCode.app'");
        // 含空格/中文：单引号包裹后 shell 不再分词
        assert_eq!(sh_quote("/Users/a b/应用.app"), "'/Users/a b/应用.app'");
        // 含单引号：拆分为 '\'' 三段
        assert_eq!(sh_quote("it's"), "'it'\\''s'");
    }

    #[test]
    #[cfg(target_os = "macos")]
    fn applescript_转义() {
        // 双引号与反斜杠需转义，单引号无需处理
        let escaped = escape_applescript("cp \"a\" \\b 'c'");
        assert_eq!(escaped, "cp \\\"a\\\" \\\\b 'c'");
    }

    #[test]
    fn 取消授权_特征识别() {
        // 本模块转换后的中文提示与 osascript 原始特征均应识别
        assert!(is_admin_cancelled("已取消管理员授权"));
        assert!(is_admin_cancelled(
            "管理员授权执行失败：osascript: User canceled. (-128)"
        ));
        // 非取消类错误不得误判（含平台不支持、普通执行失败与空串）
        assert!(!is_admin_cancelled("管理员授权执行失败：cp: 权限不足"));
        assert!(!is_admin_cancelled("当前平台暂不支持提权操作（仅 macOS / Windows）"));
        assert!(!is_admin_cancelled("管理员授权执行失败（退出码 1）"));
        assert!(!is_admin_cancelled(""));
    }

    #[test]
    fn 应用管理拦截_特征识别与防误判() {
        // cp / rsync 被 App Management 拦截时的典型 stderr（EPERM）
        assert!(is_operation_not_permitted(
            "管理员授权执行失败：cp: /Applications/ZCode.app/Contents/Resources/app.asar: Operation not permitted"
        ));
        assert!(is_operation_not_permitted(
            "rsync: rename failed: Operation not permitted"
        ));
        // EACCES（Permission denied）是另一类错误，不得误判为 TCC 拦截
        assert!(!is_operation_not_permitted("cp: xxx: Permission denied"));
        // 取消授权、普通失败与空串不得误判
        assert!(!is_operation_not_permitted("已取消管理员授权"));
        assert!(!is_operation_not_permitted("管理员授权执行失败：磁盘已满"));
        assert!(!is_operation_not_permitted(""));
    }

    #[test]
    fn 管理员错误转换_拦截时替换为设置指引() {
        // 命中拦截特征：转换为引导文案，且保留原始错误便于排查
        let out = clarify_admin_error(
            "管理员授权执行失败：cp: /Applications/ZCode.app: Operation not permitted".into(),
        );
        assert!(out.contains("应用管理"), "应含拦截原因说明：{out}");
        assert!(out.contains("隐私与安全性"), "应含系统设置路径：{out}");
        assert!(out.contains("终端"), "应含开发模式运行时的指引：{out}");
        assert!(out.contains("ZBar 想要修改 ZCode"), "应说明首次弹窗的允许方式：{out}");
        assert!(out.contains("Operation not permitted"), "应保留原始错误：{out}");

        // 独立构造的指引文案与 clarify 命中拦截时使用的底文案一致
        let hint = operation_not_permitted_hint();
        assert!(out.starts_with(&hint), "clarify 应复用同一指引文案：{out}");

        // 非拦截错误原样返回（含取消授权，交由调用方按取消处理）
        assert_eq!(
            clarify_admin_error("管理员授权执行失败：磁盘已满".into()),
            "管理员授权执行失败：磁盘已满"
        );
        assert_eq!(
            clarify_admin_error("已取消管理员授权".into()),
            "已取消管理员授权"
        );
    }
}

// ============================================================
// Windows 专属：UAC 提权链路的纯函数测试（静态正确性，随交叉检查验证）
// ============================================================

#[cfg(all(windows, test))]
mod windows_tests {
    use super::*;

    /// PowerShell 单引号字面量转义：单引号翻倍，中文与反斜杠原样保留
    #[test]
    fn ps单引号转义() {
        assert_eq!(escape_ps_single(r"C:\Temp\zbar-elevate.cmd"), r"C:\Temp\zbar-elevate.cmd");
        assert_eq!(escape_ps_single("it's"), "it''s");
        assert_eq!(escape_ps_single(r"C:\用户's\Temp"), r"C:\用户''s\Temp");
    }

    /// UAC 取消（退出码 1223 → 与 macOS 相同的中文文案）应被
    /// is_admin_cancelled 识别；普通失败不得误判。
    #[test]
    fn uac取消_文案识别() {
        assert!(is_admin_cancelled("已取消管理员授权"));
        // PowerShell 侧其它失败（退出码 1）不是取消
        assert!(!is_admin_cancelled("管理员授权执行失败（退出码 1）"));
        assert!(!is_admin_cancelled(""));
    }
}

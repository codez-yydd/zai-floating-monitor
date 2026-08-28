//! macOS ad-hoc 重签名。
//!
//! 替换 app.asar 后应用原签名失效，Gatekeeper 会拒绝启动，
//! 必须对整个 bundle 重新 ad-hoc 签名（codesign --force --deep --sign -）
//! 并做 --deep --strict 校验。常见失败原因是 quarantine 等扩展属性干扰，
//! 首次失败先 xattr -cr 清理再重试一次。
//!
//! 注意：安装/还原流程的签名命令已纳入 privilege 提权脚本内执行
//! （单会话原子替换，见 mod.rs 的 build_replace_script / build_restore_script），
//! 本模块当前无运行时调用方，保留作为当前用户身份签名的备用诊断手段。

use std::path::Path;
use std::process::Command;

/// 执行外部命令，成功返回 stdout，失败返回中文错误（含 stderr）。
#[allow(dead_code)]
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let output = Command::new(program)
        .args(args)
        .output()
        .map_err(|e| format!("调用 {program} 失败：{e}"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        let detail = if stderr.is_empty() { stdout } else { stderr };
        return Err(format!("{program} 执行失败：{detail}"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).to_string())
}

/// 清除 bundle 及其子文件的扩展属性（quarantine 等，重签失败的常见诱因）。
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn clear_xattr(bundle: &Path) -> Result<(), String> {
    run("xattr", &["-cr", &bundle.to_string_lossy()])
        .map(|_| ())
        .map_err(|e| format!("清除扩展属性失败：{e}"))
}

/// 单次"签名 + 校验"。
#[cfg(target_os = "macos")]
#[allow(dead_code)]
fn codesign_once(bundle: &Path) -> Result<(), String> {
    let bundle_s = bundle.to_string_lossy();
    run(
        "codesign",
        &["--force", "--deep", "--sign", "-", &bundle_s],
    )
    .map_err(|e| format!("ad-hoc 重签名失败：{e}"))?;
    run(
        "codesign",
        &["--verify", "--deep", "--strict", &bundle_s],
    )
    .map(|_| ())
    .map_err(|e| format!("签名校验失败：{e}"))
}

/// 重签名入口：失败先 xattr -cr 清理再重试一次。
#[cfg(target_os = "macos")]
#[allow(dead_code)]
pub fn resign_app(bundle: &Path) -> Result<(), String> {
    match codesign_once(bundle) {
        Ok(()) => Ok(()),
        Err(first) => {
            // 扩展属性干扰是最常见失败原因：清理后重试一次
            let _ = clear_xattr(bundle);
            codesign_once(bundle)
                .map_err(|second| format!("应用重签名失败（已重试）：{second}（首次错误：{first}）"))
        }
    }
}

// ---------- 非 macOS：签名流程不适用 ----------
// 保留空实现维持接口兼容（防 dead_code 警告）；Windows 替换链路
// （windows_replace_asar）无签名步骤，不会调用本组函数。

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn resign_app(_bundle: &Path) -> Result<(), String> {
    Err("当前平台暂不支持重签名（仅 macOS）".into())
}

#[cfg(not(target_os = "macos"))]
#[allow(dead_code)]
pub fn clear_xattr(_bundle: &Path) -> Result<(), String> {
    Ok(())
}

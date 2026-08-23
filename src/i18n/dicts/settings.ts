/**
 * 设置页词典：外观（透明度/语言）、开机自启、统计来源、汇率、
 * 全局快捷键各卡片的文案。
 */

export const settings = {
  "settings.title": "设置",

  // ===== 外观 =====
  "settings.panelOpacity": "面板透明度",
  "settings.opacity": "透明度",
  "settings.opacityHint":
    "调整面板背景透明度，值越低毛玻璃越透；暗色主题建议保持 60% 以上，过低时文字可能不清晰",
  "settings.language": "语言",
  "settings.langZh": "简体中文",
  "settings.langEn": "English",

  // ===== 开机自启 =====
  "settings.autostart": "开机自启",
  "settings.enable": "启用",
  "settings.applying": "应用中…",
  "settings.autostartHint":
    "登录 Windows 或 macOS 后自动启动 ZBar，面板默认保持隐藏，可从托盘打开。",
  "settings.readingState": "正在读取状态…",
  "settings.autostartReadFail": "读取开机自启状态失败：{msg}",
  "settings.autostartFailOn": "开启开机自启失败：{msg}",
  "settings.autostartFailOff": "关闭开机自启失败：{msg}",

  // ===== 统计展示来源 =====
  "settings.sources": "统计展示来源",
  "settings.instant": "即时生效",
  "settings.sourcesHint":
    "关闭后仅从统计标签和汇总中隐藏，不影响本地采集与设备同步。",
  "settings.agentZaiDesc": "Z.ai 用量与 Coding Plan 额度",
  "settings.agentCodexDesc": "OpenAI Codex CLI 用量与额度",
  "settings.agentClaudeDesc": "Claude Code 用量与订阅额度",
  "settings.agentCursorDesc": "Cursor 编辑器用量与套餐额度",

  // ===== 汇率 =====
  "settings.fxCard": "汇率",
  "settings.fxAutoNote": "自动更新中，取消勾选可手动输入",
  "settings.fxNever": "尚未联网获取",
  "settings.fxUnknownSource": "未知来源",
  "settings.fxDailyTitle": "后台每天自动联网刷新一次汇率",
  "settings.fxDaily": "每日自动更新汇率",
  "settings.updateNow": "立即更新",
  "settings.updating": "更新中…",
  "settings.updateNowTitle": "立即联网获取最新汇率（多个免费数据源自动容错）",
  "settings.fxNote":
    "模型价格只存美元，人民币花费按此汇率自动折算（价格设置页的 ¥ 视图同源）。",

  // ===== 全局快捷键 =====
  "settings.shortcut": "全局快捷键",
  "settings.shortcutHint":
    "唤起/隐藏面板。格式如 alt+shift+z（修饰键用 ctrl/alt/shift/cmd，主键用字母/数字）。",
  "settings.apply": "应用",
  "settings.applied": "已应用 ✓",

  // ===== 关于与更新 =====
  "settings.aboutCard": "关于与更新",
  "settings.currentVersion": "当前版本",
  "settings.checkUpdate": "检查更新",
  "settings.checking": "检查中…",
  "settings.upToDate": "已是最新版本",
  "settings.newVersion": "发现新版本 {v}",
  "settings.downloadInstall": "下载并安装",
  "settings.downloading": "下载中 {pct}%",
  "settings.installing": "安装中…",
  "settings.restartUpdate": "重启并更新",
  "settings.updateReady": "新版本 {v} 已下载，重启即可完成更新",
  "settings.backgroundDownloading": "正在后台下载更新…",
  "settings.updateFailed": "更新失败：{msg}",
  "settings.openDownloadPage": "打开下载页",
  "settings.updateHint":
    "每小时自动检查并在后台下载更新；下载完成后设置入口会显示红点。更新包带签名校验，更新源为 GitHub / Gitee 双仓库，自动依次尝试。",

  // ===== 多智谱账号切换 =====
  "settings.accountsCard": "ZCode 账号切换",
  "settings.accountsHint":
    "捕获当前 ZCode 登录为快照后，可在多个账号间一键切换（自动退出并重启 ZCode，切换前备份、失败自动回滚）。快照仅存本机，不参与同步。",
  "settings.accountsCurrent": "当前",
  "settings.accountsUnknown": "未识别",
  "settings.accountsCapture": "捕获当前登录",
  "settings.accountsCapturing": "捕获中…",
  "settings.accountsCapturedNew": "已保存新账号快照 ✓",
  "settings.accountsCapturedUpdate": "已更新该账号的快照 ✓",
  "settings.accountsSwitch": "切换",
  "settings.accountsSwitching": "切换中…",
  "settings.accountsSwitched": "已切换到「{name}」✓",
  "settings.accountsRelaunchFail": "（ZCode 自动重启失败，请手动打开）",
  "settings.accountsConfirmTitle": "切换账号",
  "settings.accountsConfirmDesc":
    "将退出并重启 ZCode 桌面应用，登录态切换为「{name}」。切换前会自动备份当前登录，失败时自动回滚。",
  "settings.accountsRemoveConfirm":
    "仅删除本应用保存的快照，不影响 ZCode 当前登录。",
  "settings.accountsRename": "重命名",
  "settings.accountsRenameDesc": "为该账号快照设置一个好认的名字。",
  "settings.accountsEmpty":
    "还没有账号快照。在 ZCode 客户端登录账号后点「捕获当前登录」保存，之后在另一个账号登录再捕获一次，即可一键切换。",
  "settings.accountsLoadFail": "读取账号列表失败：{msg}",
};

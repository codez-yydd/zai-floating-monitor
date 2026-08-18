/**
 * 设置页词典：外观（透明度/语言）、开机自启、统计来源、Coding Plan、
 * Cursor 统计、全局快捷键各卡片的文案。
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

  // ===== Coding Plan 额度监控 =====
  "settings.tokenPh": "粘贴 Coding Plan API Token",
  "settings.endpoint": "端点",
  "settings.endpointCn": "🇨🇳 国内",
  "settings.endpointGlobal": "🌐 国际",
  "settings.endpointHint": "Token 从智谱开放平台获取。国内用户选「国内」端点。",

  // ===== Cursor 统计 =====
  "settings.cursorStats": "Cursor 统计",
  "settings.debug": "诊断",
  "settings.debugging": "诊断中…",
  "settings.debugFailed": "诊断失败：{msg}",
  "settings.debugSource": "来源",
  "settings.debugDbFound": "已找到",
  "settings.debugDbMissing": "未找到",
  "settings.debugResponse": "响应",
  "settings.test": "测试连接",
  "settings.testing": "测试中…",
  "settings.connectedEmail": "✓ 已连接：{email}",
  "settings.connectedEmailPlan": "✓ 已连接：{email}（{plan}）",
  "settings.connectedName": "✓ 已连接：{name}",
  "settings.authOk": "✓ 认证成功",
  "settings.auth": "认证",
  "settings.authAuto": "自动（读 Cursor 应用）",
  "settings.authManual": "手动 Cookie",
  "settings.cookiePh": "粘贴 cursor.com 请求的 Cookie 头",

  // ===== 汇率 =====
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
  "settings.cursorAutoHint":
    "自动读取 Cursor 应用的本地登录凭据。请确保 Cursor 已安装并登录。",

  // ===== 全局快捷键 =====
  "settings.shortcut": "全局快捷键",
  "settings.shortcutHint":
    "唤起/隐藏面板。格式如 alt+shift+z（修饰键用 ctrl/alt/shift/cmd，主键用字母/数字）。",
  "settings.apply": "应用",
  "settings.applied": "已应用 ✓",
};

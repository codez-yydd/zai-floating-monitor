/**
 * 统计域词典：统计面板（标签/工具栏/设备筛选）、Coding Plan 额度面板、
 * 各 Agent 用量面板（Codex / Claude / Cursor）的文案。
 */

export const stats = {
  // ===== 统计面板顶栏 =====
  "stats.tab.summary": "汇总",
  "stats.reports": "报表",
  "stats.syncOn": "设备同步",
  "stats.syncOff": "配置设备同步",
  "stats.settings": "设置",
  "stats.pin": "常驻置顶",
  "stats.unpin": "取消常驻",
  "stats.priceSettings": "价格设置",
  "stats.sourcesAria": "统计来源",

  // ===== 设备筛选 =====
  "stats.deviceFilter": "筛选设备",
  "stats.deviceAll": "全部",
  "stats.deviceLocal": "本机",
  "stats.deviceLocalName": "本机（{name}）",

  // ===== Coding Plan 额度面板 =====
  "quota.title": "Coding Plan 额度监控",
  "quota.configHint": "请在 ZCode 客户端登录 Coding Plan 订阅，登录后自动读取额度",
  "quota.failed": "额度查询失败：{msg}",
  "quota.refresh": "刷新额度",
  "quota.todayDelta": "↑今日 {pct}%",
  "quota.allAccounts": "全部账号",
  "quota.quotaFail": "额度查询失败",
  "quota.weekShort": "周剩",
  "quota.hour5Short": "5h",

  // ===== Agent 用量面板（Codex / Claude / Cursor 共用结构）=====
  "stats.rateLimits": "额度",
  "stats.noDataFor": "未获取到 {name} 数据",
  "stats.codexNotFound": "未检测到 Codex",
  "stats.codexNotFoundHint":
    "请安装并使用 OpenAI Codex CLI 产生本地会话记录\n（~/.codex/sessions）后再查看",
  "stats.claudeNotFound": "未检测到 Claude Code",
  "stats.claudeNotFoundHint":
    "请安装并使用 Anthropic Claude Code 产生本地会话记录\n（~/.claude/projects）后再查看",
  // Claude 订阅额度增量窗口（API 返回该窗口才有值，缺省不渲染）
  "stats.claudeOpusWeekly": "Opus 周额度",
  "stats.claudeSonnetWeekly": "Sonnet 周额度",
  "stats.claudeExtraUsage": "超额消费",
  "stats.claudeOtherAccounts": "其他账号",
  "stats.cursorOtherAccounts": "其他账号",
  "stats.kimiNotFound": "未检测到 Kimi Code",
  "stats.kimiNotFoundHint":
    "请安装并使用 Kimi Code CLI 产生本地会话记录\n（~/.kimi-code/sessions）后再查看",
  "stats.boosterBalance": "加油包余额",
  "stats.boosterMonthlyUsed": "本月已用 ¥{amount}",
  "stats.kimiMonthlyQuota": "月总额度",

  // ===== Cursor 面板 =====
  "cursor.notLoggedIn": "未检测到 Cursor 登录",
  "cursor.loginHint": "请在 Cursor 应用中登录，登录后自动读取本地用量",
  "cursor.account": "账户",
  "cursor.unknown": "未知",
  "cursor.planQuota": "套餐额度",
  "cursor.noQuotaData": "暂无额度数据",
  "cursor.onDemand": "按需用量",
  "cursor.cycle": "周期 {date}",
  "cursor.resetDate": "重置 {date}",
  "cursor.tokenSpend": "Token 花费",
  "cursor.selectedRange": "所选时间范围",
  "cursor.byModel": "按模型",
  "cursor.eventsFailed": "Token 明细拉取失败：{msg}",
  "cursor.noEvents": "所选时间范围内暂无 Token 使用明细",
};

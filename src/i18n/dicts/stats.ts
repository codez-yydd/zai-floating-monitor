/**
 * 统计域词典：统计面板（标签/工具栏/设备筛选）、Coding Plan 额度面板、
 * 各 Agent 用量面板（Codex / Claude / Cursor）的文案。
 */

export const stats = {
  // ===== 统计面板顶栏 =====
  "stats.tab.summary": "汇总",
  "stats.tab.speed": "速度",
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
  "stats.kimiOtherAccounts": "其他账号",
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

// ===== 模型速度统计面板（ModelSpeedPanel，"速度"tab）=====
// V5 行块骨架 + 明细平铺：常态每行 = 模型名 + 速度值 + 相对速度条 + 3 行 ×
// 2 列明细网格（速度/首字/耗时/输入/输出/请求，全部数值直接可见、无需悬停
// 依赖）。明细格 = 11px 灰标签 + 12px 黑值，统计口径直接写进值串。速度分位
// 在 UI 中使用“慢 / 平均 / 快”直白标签（p50 是中位数、非总体加权均速；
// 文案按用户习惯用“平均”）。键集与 en 保持
// 一致（satisfies 约束）。
export const speed = {
  "speed.title": "模型速度",
  "speed.subtitle": "按平均速度比较模型的流式输出表现",
  "speed.rangeToday": "今日",
  "speed.range7d": "7天",
  "speed.rangeLabel": "速度统计范围",
  "speed.fastest": "最快平均速度",
  "speed.fastestHint": "当前范围内",
  "speed.autoRefresh": "每 30 秒自动更新",
  "speed.overviewModels": "模型",
  "speed.overviewRequests": "请求",
  "speed.overviewMean": "平均速度",
  "speed.overviewSuccess": "成功率",
  "speed.ranking": "模型排行",
  "speed.modelCount": "{n} 个模型",
  "speed.speedSamples": "速度样本 {n}",
  "speed.samplesNone": "暂无速度样本",
  "speed.providerUnknown": "未标注来源",
  "speed.relativeSpeed": "相对最快模型 {pct}%",
  "speed.relativeLabel": "相对最快模型",
  "speed.averagePeak": "平均 {avg} · 峰值 {max}",
  "speed.averageSlower": "平均 {avg} · 较慢 {slow}",
  "speed.requestSummary": "{n} 次 · 成功率 {pct}",
  "speed.p10": "慢",
  "speed.p50": "平均",
  "speed.p90": "快",
  "speed.ttft": "首字",
  "speed.latency": "耗时",
  "speed.inputShort": "输入",
  "speed.outputShort": "输出",
  "speed.latencyShort": "耗时",
  "speed.samplesShort": "请求",
  "speed.waitingData": "等待本地请求数据",
  "speed.live": "实时",
  "speed.loadFailed": "速度数据加载失败",
  "speed.retry": "重试",
  "speed.empty": "所选时间范围内暂无模型请求",
  "speed.emptyHint": "在 ZCode 中发起对话后自动统计",
  "speed.fail": "模型速度统计查询失败：{msg}",
  // 数据截至标识（每次查询成功后更新，Ns 随时间自走）
  "speed.updatedAt": "数据截至 {time} · {ago}s 前",
  "speed.updatedHint": "最近一次查询成功时刻；每 30 秒自动刷新",
  "speed.refresh": "立即刷新",
};

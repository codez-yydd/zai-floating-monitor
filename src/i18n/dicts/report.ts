/**
 * 用量报告页词典：页内展示、额度快照、报告结论，以及导出 Markdown 与
 * 文件名（报告语言跟随 UI 语言，无独立选项）。
 */

export const report = {
  // ===== 页面框架 =====
  "report.title": "用量报告",
  "report.refresh": "刷新报告",
  // 时间范围选择复用共享 RangePicker（range.* 词典）
  "report.allDevices": "全部设备",
  "report.loading": "正在整理用量数据…",
  "report.emptyTitle": "当前范围暂无用量",
  "report.emptyHint": "请确认 Agent 已开启，并在所选时间范围内产生过请求。",
  "report.refreshing": "正在刷新最新数据…",

  // ===== 指标卡 =====
  "report.cnyHint": "人民币折算",
  "report.usdHint": "美元原价",
  "report.tokenHint": "当前可见 Agent",
  "report.requestsHint": "调用次数",
  "report.activeAgents": "活跃 Agent",
  "report.agentsHintOne": "本范围仅 1 个来源",
  "report.agentsHint": "本范围有用量的来源",

  // ===== 区块 =====
  "report.noTrend": "当前来源没有可绘制的趋势数据",
  "report.agentDist": "Agent 分布",
  "report.modelRank": "模型排行",
  "report.noModels": "当前 Agent 没有返回模型明细。",

  // ===== 报告结论 =====
  "report.conclusion": "报告结论",
  "report.mainModel": "主力模型：",
  "report.mainModelLine": "{model}（{agent}），{tokens} Token。",
  "report.peakWindow": "峰值时段：",
  "report.peakWindowLine": "{label}，{value}。",
  "report.unpricedWarn":
    "有 {tokens} Token 未配置价格，花费统计会偏低；可到价格设置补充模型价格。",
  "report.allPriced": "当前用量的模型均已配置价格，花费统计可用于横向比较。",

  // ===== 额度快照 =====
  "report.quotaSnapshot": "Agent 额度快照",
  "report.quotaScope": "已开启且有数据的 Agent",
  "report.accountLevel": "账户级",
  "report.localRealtime": "本机实时",
  "report.quotaSourceNote":
    "Z.ai 额度来自历史快照；Codex、Claude、Cursor 为本机实时额度接口。",
  "report.resetUnknown": "重置时间未知",
  "report.resetDays": "约 {n} 天后重置",
  "report.resetHours": "约 {n} 小时后重置",
  // 双开展示的简短形态（后接 " · 重置于 MM-DD HH:mm"，避免"重置"措辞重复）
  "report.resetInDays": "约 {n} 天后",
  "report.resetInHours": "约 {n} 小时后",

  // 额度窗口标签（labelKey，渲染/导出时查词典）
  "report.q.weeklyCurrent": "周额度当前",
  "report.q.hour5Current": "5h 当前",
  "report.q.weeklyPeak": "周额度峰值",
  "report.q.hour5Peak": "5h 峰值",
  "report.q.mcp": "MCP",
  "report.q.hour5": "5h",
  "report.q.weekly": "本周",
  "report.q.auto": "Auto",
  "report.q.api": "API",
  "report.q.plan": "套餐",
  "report.q.onDemand": "按需",

  // ===== 底部操作 =====
  "report.copy": "复制",
  "report.copied": "已复制到剪贴板 ✓",
  "report.savedOpened": "已保存并在文件夹打开 ✓",

  // ===== 数据说明 =====
  "report.noteCursorDaily":
    "Cursor 官方明细按日返回，小时趋势未混入 Cursor，Agent 汇总仍包含它。",

  // ===== 导出文件名 =====
  "report.file.daily": "日报-",
  "report.file.custom": "用量报告-",

  // ===== 导出 Markdown 全文 =====
  "report.md.daily": "用量日报",
  "report.md.custom": "用量报告",
  "report.md.noData": "（暂无数据）",
  "report.md.summaryLine": "总花费 {cost}｜Token {tokens}｜请求 {requests} 次",
  "report.md.agentLine": "{label}：{cost}｜{tokens} Token｜{requests} 次请求",
  "report.md.top5": "模型 TOP5",
  "report.md.modelLine": "{agent} / {model}：{cost}｜{tokens} Token",
  "report.md.tokenPeak": "Token 峰值：{label}，{tokens} Token",
  "report.md.quotaSnapshot": "额度快照",
  "report.md.quotaLine": "{label}（{scope}）：「{windows}」",
  "report.md.quotaReset": "｜{text}",
  "report.md.notes": "说明",
  "report.md.footer": "由 ZBar 自动生成 · {date}",
};

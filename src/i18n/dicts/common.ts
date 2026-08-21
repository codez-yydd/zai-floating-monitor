/**
 * 通用词典：跨页面复用的基础文案（加载、保存、剩余、Token 构成等）与时间范围预设。
 */

export const common = {
  // 基础状态
  "common.loading": "加载中…",
  "common.loadingUsage": "加载{name}用量…",
  "common.switchLanguage": "切换语言",
  "common.refresh": "刷新",
  "common.cancel": "取消",
  "common.confirm": "确认",
  "common.delete": "删除",
  "common.show": "显示",
  "common.hide": "隐藏",

  // 保存动作（多页面按钮复用）
  "common.save": "保存",
  "common.saving": "保存中…",
  "common.saved": "已保存 ✓",

  // 额度 / 指标通用
  "common.refreshIn": "{time} 后刷新",
  "common.remaining": "剩 {pct}%",
  "common.cacheHit": "缓存命中 {pct}",
  "common.hour5": "5小时",
  "common.weekly": "本周",

  // 用量页通用指标
  "common.totalCost": "总花费",
  "common.totalTokens": "总 Token",
  "common.cost": "花费",
  "common.requests": "请求",
  "common.requestCount": "请求次数",
  "common.output": "输出",
  "common.cacheRate": "缓存率",
  "common.tokenComposition": "Token 构成",
  "common.input": "输入",
  "common.cache": "缓存",
  "common.reasoning": "推理",
  "common.modelUsage": "模型用量",
  "common.noPrice": "未配置价格",

  // 趋势图
  "common.usageTrend": "用量趋势",
  "common.trendFlat": "持平",
  "common.trendNew": "新增",
  "common.trendVsHour": "最新小时 vs 上一小时",
  "common.trendVsDay": "最新日 vs 上一日",

  // 设备下拉选项
  "common.deviceOption": "{name}（{id}）",

  // 时间范围预设
  "range.today": "今日",
  "range.24h": "24h",
  "range.7d": "7天",
  "range.30d": "30天",
  "range.custom": "自定义",

  // 日期选择器星期表头（空格分隔，渲染时 split）
  "date.weekdays": "日 一 二 三 四 五 六",
};

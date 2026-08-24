/**
 * 周额度对比页词典：自然周分组柱状图、周明细、全部周列表的文案。
 */

export const compare = {
  "compare.title": "周额度对比",
  "compare.device": "设备",
  "compare.all": "全部（汇总）",
  "compare.remoteHistoryFailed": "远端额度历史获取失败：{msg}",
  "compare.emptyTitle": "暂无周额度数据",
  "compare.emptyHint": "应用开启后会自动采样各订阅的周额度\n请保持运行以积累数据",
  "compare.chartTitle": "各订阅周额度已用比例",
  "compare.chartHint": "柱高 = 该自然周内各订阅周额度的已用峰值。",
  "compare.allPeriods": "全部周",
  "compare.thisWeek": "本周",
  "compare.selectedWeek": "选中周",
  "compare.weekRange": "{from} ~ {to}",
  "compare.seriesHint": "各订阅在本周的额度已用峰值；「无数据」= 该周无采样记录",
  "compare.peakShort": "峰值",
  "compare.noDataShort": "无数据",
  "compare.peakUsedPct": "周内已用峰值 {pct}%",
  "compare.noSample": "该周无采样",
  "compare.legacyAccount": "旧快照",
  "compare.legacyAccountHint": "多账号区分上线前记录的历史采样，无法归属到具体账号",
  "compare.samples": "采样 {count} 条",
  "compare.tokensOfAgents": "Agent 实际 Token 数量（不是额度百分比）",
  "compare.tokenShort": "Token",
  "compare.requestsCount": "{count} 次请求",
  "compare.noUsage": "该周暂无已启用 Agent 的用量",
  "compare.footer":
    "额度% = 各订阅周额度窗口在自然周内的已用峰值；Token = 该自然周内已启用 Agent 的实际用量。两者不是同一指标。",
};

/**
 * 价格设置页词典：货币切换、模型价格编辑、内置参考表差异面板的文案。
 */

export const pricing = {
  "pricing.loading": "加载价格配置…",
  "pricing.title": "价格设置",
  "pricing.cny": "¥ 人民币",
  "pricing.usd": "$ 美元",
  "pricing.unitHint": "单位：$/百万 token。人民币按汇率 {rate} 自动折算。",
  "pricing.checkUpdates": "检查价格更新",
  "pricing.missingCount": "{count} 个模型未配价",
  "pricing.upToDate": "已是最新 ✓",

  // 差异面板
  "pricing.diffTitle": "价格更新 · 内置参考表",
  "pricing.diffHint":
    "参考价离线对比不联网。带 ≈ 标记的是变体名匹配的基础模型参考价，应用前请确认。",
  "pricing.missingWarn":
    "以下 {count} 个模型实际在用但未配置价格（花费按 0 计）：",
  "pricing.addPriceBelow": "请在下方模型列表中手动补价",
  "pricing.badgeNew": "新增",
  "pricing.badgeChanged": "变动",
  "pricing.refFrom":
    "参考价取自基础模型 {model}（同系变体），应用前请确认价格",
  "pricing.old": "旧",
  "pricing.unpriced": "未配价",
  "pricing.noDiff": "无差异，价格已是最新",
  "pricing.collapse": "收起",
  "pricing.applying": "应用中…",
  "pricing.applySelected": "应用选中",
  "pricing.applySelectedCount": "应用选中 ({count})",
  "pricing.noModels": "暂无模型数据。请确认 Z.ai 已产生使用记录。",
};

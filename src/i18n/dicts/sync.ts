/**
 * 设备同步页词典：注册连接、同步模式、数据管理与三个对话框的文案。
 */

export const sync = {
  "sync.loading": "加载同步配置…",
  "sync.title": "设备同步",

  // 相对时间
  "sync.never": "从未",
  "sync.justNow": "刚刚",
  "sync.minAgo": "{n} 分钟前",
  "sync.hourAgo": "{n} 小时前",
  "sync.dayAgo": "{n} 天前",

  // 注册连接卡片
  "sync.connectTitle": "连接到同步服务器",
  "sync.connectHint":
    "先用 Docker 部署 zbar-sync 服务，从启动日志复制 Master Token。",
  "sync.serverUrl": "服务器地址",
  "sync.masterLabel": "准入凭证 (Master Token)",
  "sync.masterPh": "docker logs 中的 MASTER_TOKEN",
  "sync.deviceName": "设备名称",
  "sync.namePh": "如：work / home",
  "sync.httpWarn":
    "⚠️ HTTP 明文传输，建议内网使用或配置 HTTPS 反向代理。",
  "sync.connecting": "连接中…",
  "sync.connect": "连接并注册",
  "sync.needMaster": "请先填写 Master Token（从服务器日志获取）",
  "sync.needMasterAuto": "配置自动清理需要 Master Token",

  // 状态卡片
  "sync.server": "服务器",
  "sync.lastSync": "上次同步",
  "sync.pending": "待上传",
  "sync.recordsCount": "{count} 条",
  "sync.uploadCursor": "已传游标",
  "sync.mode": "同步模式",
  "sync.manual": "手动",
  "sync.auto": "自动",
  "sync.interval": "间隔（秒）",
  "sync.syncing": "同步中…",
  "sync.syncNow": "立即同步",
  "sync.saveMode": "保存模式",
  "sync.disconnect": "断开",

  // 操作结果提示
  "sync.uploaded": "已上传 {count} 条",
  "sync.deletedBoth": "已删除 {records} 条记录、{devices} 个设备",
  "sync.deleted": "已删除 {count} 条记录",
  "sync.merged": "已合并 {count} 条记录到「{name}」",
  "sync.targetDevice": "目标设备",
  "sync.renamed": "已改名为「{name}」",
  "sync.deviceMissing": "设备不存在或已被删除",

  // 数据管理卡片
  "sync.dataMgmt": "数据管理",
  "sync.totalRecords": "共 {count} 条",
  "sync.masterForCleanup": "Master Token（操作清理用）",
  "sync.pasteMasterPh": "粘贴 Master Token",
  "sync.autoCleanup": "自动清理",
  "sync.on": "已开启",
  "sync.off": "已关闭",
  "sync.keepDays": "保留天数",
  "sync.localBadge": "本机",
  "sync.rename": "改名",
  "sync.merge": "合并",
  "sync.mergeInto": "合并到其他设备",
  "sync.deleteDeviceData": "删除此设备数据",
  "sync.cleanByTime": "按时间清理",
  "sync.clearAll": "全部清空",
  "sync.reset": "重置",

  // 合并对话框
  "sync.mergeTitle": "合并设备",
  "sync.mergeDesc": "将「{name}（{id}）」的 {count} 条记录合并到：",
  "sync.mergeLocalWarn":
    "合并到本机后，被合并的历史数据在本机\"全部汇总\"视图中可能不可见。建议改合并到该设备当前正在用的实例。",
  "sync.mergeWarn":
    "来源设备的记录会转移到目标设备，来源设备将被删除，不可恢复。若来源设备仍在某台机器上同步，请到那台机器\"断开\"并重新注册。",
  "sync.mergeConfirm": "确认合并",

  // 改名对话框
  "sync.renameTitle": "设备改名",
  "sync.renameDesc": "修改「{name}（{id}）」的名称。",

  // 清理确认对话框
  "sync.confirmTitleDevice": "删除设备数据",
  "sync.confirmTitleReset": "重置服务器",
  "sync.confirmDescDevice": "将删除该设备的全部明细，不可恢复。",
  "sync.confirmDescBefore":
    "将删除 {days} 天前的所有数据，趋势图历史范围会缩短。不可恢复。",
  "sync.confirmDescAll": "将清空所有用量数据（保留设备注册），不可恢复。",
  "sync.confirmDescReset": "将清空所有数据并删除所有设备，回到初始状态。不可恢复。",
  "sync.confirmDelete": "确认删除",
};

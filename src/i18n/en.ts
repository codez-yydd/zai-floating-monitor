/**
 * 英文词典：typeof zh 约束保证与中文键集完全一致（缺键/多键编译报错）。
 * 译文追求自然简洁，窄面板处用短词（如 Custom 而非 Customize）；
 * 品牌名（ZBar、Claude、Codex、Gemini、Cursor 等）不翻译。
 */
import { common } from "./dicts/common";
import { layout } from "./dicts/layout";
import { stats } from "./dicts/stats";
import { summary } from "./dicts/summary";
import { pricing } from "./dicts/pricing";
import { sync } from "./dicts/sync";
import { compare } from "./dicts/compare";
import { report } from "./dicts/report";
import { settings } from "./dicts/settings";
import { zh } from "./zh";

export const en: typeof zh = {
  ...({
    // 基础状态
    "common.loading": "Loading…",
    "common.loadingUsage": "Loading {name} usage…",
    "common.switchLanguage": "Switch language",
    "common.refresh": "Refresh",
    "common.cancel": "Cancel",
    "common.confirm": "Confirm",
    "common.show": "Show",
    "common.hide": "Hide",

    // 保存动作
    "common.save": "Save",
    "common.saving": "Saving…",
    "common.saved": "Saved ✓",

    // 额度 / 指标通用
    "common.refreshIn": "Refreshes in {time}",
    "common.remaining": "{pct}% left",
    "common.cacheHit": "Cache hit {pct}",
    "common.hour5": "5h",
    "common.weekly": "Week",

    // 用量页通用指标
    "common.totalCost": "Total cost",
    "common.totalTokens": "Total tokens",
    "common.cost": "Cost",
    "common.requests": "Requests",
    "common.requestCount": "Requests",
    "common.output": "Output",
    "common.cacheRate": "Cache rate",
    "common.tokenComposition": "Token breakdown",
    "common.input": "Input",
    "common.cache": "Cache",
    "common.reasoning": "Reasoning",
    "common.modelUsage": "Models",
    "common.noPrice": "No price set",

    // 趋势图
    "common.usageTrend": "Usage trend",
    "common.trendFlat": "Flat",
    "common.trendNew": "New",
    "common.trendVsHour": "Last hour vs previous",
    "common.trendVsDay": "Last day vs previous",

    // 设备下拉选项
    "common.deviceOption": "{name} ({id})",

    // 时间范围预设
    "range.today": "Today",
    "range.24h": "24h",
    "range.7d": "7d",
    "range.30d": "30d",
    "range.custom": "Custom",

    // 日期选择器星期表头
    "date.weekdays": "Su Mo Tu We Th Fr Sa",
  } satisfies typeof common),

  ...({
    "layout.back": "← Back",
    "layout.themeLight": "Switch to light",
    "layout.themeDark": "Switch to dark",
    // 语言切换按钮的目标语言字形（en 界面下点击切回中文）
    "layout.langGlyph": "中",
  } satisfies typeof layout),

  ...({
    // 统计面板顶栏
    "stats.tab.summary": "Summary",
    "stats.compare": "Quota compare",
    "stats.report": "Usage report",
    "stats.syncOn": "Device sync",
    "stats.syncOff": "Set up device sync",
    "stats.settings": "Settings",
    "stats.pin": "Always on top",
    "stats.unpin": "Disable always on top",
    "stats.priceSettings": "Pricing",
    "stats.sourcesAria": "Data sources",

    // 设备筛选
    "stats.deviceFilter": "Filter devices",
    "stats.deviceAll": "All",
    "stats.deviceLocal": "Local",
    "stats.deviceLocalName": "Local ({name})",

    // Coding Plan 额度面板
    "quota.title": "Coding Plan quota",
    "quota.configHint": "Sign in to a Coding Plan in the ZCode client; quota is read automatically",
    "quota.failed": "Failed to load quota: {msg}",
    "quota.refresh": "Refresh quota",
    "quota.todayDelta": "+{pct}% today",

    // Agent 用量面板
    "stats.rateLimits": "Quotas",
    "stats.noDataFor": "No {name} data",
    "stats.codexNotFound": "No Codex detected",
    "stats.codexNotFoundHint":
      "Install and run OpenAI Codex CLI to create local sessions\n(~/.codex/sessions), then check back",
    "stats.claudeNotFound": "No Claude Code detected",
    "stats.claudeNotFoundHint":
      "Install and run Claude Code to create local sessions\n(~/.claude/projects), then check back",

    // Cursor 面板
    "cursor.notLoggedIn": "Cursor not signed in",
    "cursor.loginHint":
      "Sign in to the Cursor app; local usage is read automatically",
    "cursor.account": "Account",
    "cursor.unknown": "Unknown",
    "cursor.planQuota": "Plan quotas",
    "cursor.noQuotaData": "No quota data",
    "cursor.onDemand": "On-demand",
    "cursor.cycle": "Cycle {date}",
    "cursor.resetDate": "Resets {date}",
    "cursor.tokenSpend": "Token spend",
    "cursor.selectedRange": "Selected range",
    "cursor.byModel": "By model",
    "cursor.eventsFailed": "Failed to load token usage: {msg}",
    "cursor.noEvents": "No token usage in the selected range",
  } satisfies typeof stats),

  ...({
    "summary.totalCost": "Total cost",
    "summary.sources": "{count} sources",
    "summary.totalTokens": "Total tokens",
    "summary.costDist": "Cost split",
    "summary.quotaMonitor": "Quotas",
    "summary.resetDate": "Resets {date}",
    "summary.noToken": "Coding Plan not signed in",
    "summary.quotaFailed": "Quota unavailable",
    "summary.noData": "No data",
    "summary.loadFailed": "Failed to load",
    "summary.notLoggedIn": "Not signed in",
    "summary.noQuotaData": "No quota data",
  } satisfies typeof summary),

  ...({
    "pricing.loading": "Loading pricing…",
    "pricing.title": "Pricing",
    "pricing.cny": "¥ CNY",
    "pricing.usd": "$ USD",
    "pricing.unitHint": "Unit: $/M tokens. CNY converted at {rate}.",
    "pricing.checkUpdates": "Check updates",
    "pricing.missingCount": "{count} models unpriced",
    "pricing.upToDate": "Up to date ✓",

    "pricing.diffTitle": "Price updates · built-in reference",
    "pricing.diffHint":
      "Compared offline against the built-in reference. Items marked ≈ inherit a base model's price by variant name — verify before applying.",
    "pricing.missingWarn":
      "{count} models in use have no price (their cost counts as 0):",
    "pricing.addPriceBelow": "Add prices manually in the list below",
    "pricing.badgeNew": "New",
    "pricing.badgeChanged": "Changed",
    "pricing.refFrom":
      "Reference price from base model {model} (same family); verify before applying",
    "pricing.old": "Old",
    "pricing.unpriced": "unpriced",
    "pricing.noDiff": "No differences — prices are up to date",
    "pricing.collapse": "Collapse",
    "pricing.applying": "Applying…",
    "pricing.applySelected": "Apply selected",
    "pricing.applySelectedCount": "Apply selected ({count})",
    "pricing.noModels": "No models yet. Make sure Z.ai has usage records.",
  } satisfies typeof pricing),

  ...({
    "sync.loading": "Loading sync settings…",
    "sync.title": "Device Sync",

    "sync.never": "Never",
    "sync.justNow": "Just now",
    "sync.minAgo": "{n} min ago",
    "sync.hourAgo": "{n} h ago",
    "sync.dayAgo": "{n} d ago",

    "sync.connectTitle": "Connect to sync server",
    "sync.connectHint":
      "Deploy the zbar-sync server with Docker first, then copy the Master Token from its logs.",
    "sync.serverUrl": "Server URL",
    "sync.masterLabel": "Credential (Master Token)",
    "sync.masterPh": "MASTER_TOKEN from docker logs",
    "sync.deviceName": "Device name",
    "sync.namePh": "e.g. work / home",
    "sync.httpWarn":
      "⚠️ HTTP is unencrypted. Keep it on a LAN or add an HTTPS reverse proxy.",
    "sync.connecting": "Connecting…",
    "sync.connect": "Connect & register",
    "sync.needMaster": "Enter the Master Token first (from server logs)",
    "sync.needMasterAuto": "Auto-cleanup requires the Master Token",

    "sync.server": "Server",
    "sync.lastSync": "Last sync",
    "sync.pending": "Pending",
    "sync.recordsCount": "{count} records",
    "sync.uploadCursor": "Last upload",
    "sync.mode": "Sync mode",
    "sync.manual": "Manual",
    "sync.auto": "Auto",
    "sync.interval": "Interval (s)",
    "sync.syncing": "Syncing…",
    "sync.syncNow": "Sync now",
    "sync.saveMode": "Save mode",
    "sync.disconnect": "Disconnect",

    "sync.uploaded": "Uploaded {count} records",
    "sync.deletedBoth": "Deleted {records} records and {devices} devices",
    "sync.deleted": "Deleted {count} records",
    "sync.merged": "Merged {count} records into \"{name}\"",
    "sync.targetDevice": "target device",
    "sync.renamed": "Renamed to \"{name}\"",
    "sync.deviceMissing": "Device not found or already deleted",

    "sync.dataMgmt": "Data management",
    "sync.totalRecords": "{count} in total",
    "sync.masterForCleanup": "Master Token (for cleanup)",
    "sync.pasteMasterPh": "Paste Master Token",
    "sync.autoCleanup": "Auto cleanup",
    "sync.on": "On",
    "sync.off": "Off",
    "sync.keepDays": "Retention days",
    "sync.localBadge": "Local",
    "sync.rename": "Rename",
    "sync.merge": "Merge",
    "sync.mergeInto": "Merge into another device",
    "sync.deleteDeviceData": "Delete this device's data",
    "sync.cleanByTime": "Clean by time",
    "sync.clearAll": "Clear all",
    "sync.reset": "Reset",

    "sync.mergeTitle": "Merge devices",
    "sync.mergeDesc": "Merge {count} records from \"{name} ({id})\" into:",
    "sync.mergeLocalWarn":
      "After merging into this device, the merged history may not appear in this device's \"All devices\" view. Prefer merging into the instance that device currently uses.",
    "sync.mergeWarn":
      "Records move to the target device and the source device is deleted — this cannot be undone. If the source device still syncs on another machine, disconnect and re-register there.",
    "sync.mergeConfirm": "Merge",

    "sync.renameTitle": "Rename device",
    "sync.renameDesc": "Rename \"{name} ({id})\".",

    "sync.confirmTitleDevice": "Delete device data",
    "sync.confirmTitleReset": "Reset server",
    "sync.confirmDescDevice": "All of this device's records will be deleted. This cannot be undone.",
    "sync.confirmDescBefore":
      "Deletes all data older than {days} days and shortens trend history. Cannot be undone.",
    "sync.confirmDescAll": "Clears all usage data (device registrations kept). Cannot be undone.",
    "sync.confirmDescReset": "Clears all data and deletes all devices, returning to a clean slate. Cannot be undone.",
    "sync.confirmDelete": "Delete",
  } satisfies typeof sync),

  ...({
    "compare.title": "Quota compare",
    "compare.device": "Device",
    "compare.all": "All (combined)",
    "compare.remoteHistoryFailed": "Failed to load remote quota history: {msg}",
    "compare.emptyTitle": "No weekly quota history yet",
    "compare.emptyHint":
      "Weekly quotas are sampled automatically while the app runs.\nKeep it running to accumulate history.",
    "compare.chartTitle": "Z.ai weekly quota used",
    "compare.percentUnit": "Unit: used %",
    "compare.chartHint":
      "Bar height = the used percentage of Z.ai's weekly quota at period end. It is not a token percentage; actual token consumption is shown below.",
    "compare.barTitle":
      "{date}: {end}% used at period end · {peak}% sampled peak",
    "compare.subscriptionTitle": "All available subscription quotas",
    "compare.subscriptionHint":
      "Z.ai shows weekly quota; Codex/Claude show 5h and weekly quotas; Cursor shows Auto/API for the current billing cycle. Numbers are used percentages; bars show remaining percentages. Different subscriptions are not added together.",
    "compare.quotaUsed": "{pct}% used",
    "compare.sampledAt": "sampled {time}",
    "compare.noQuotaSnapshot": "No quota snapshot available",
    "compare.cursorAuto": "Auto",
    "compare.cursorApi": "API",
    "compare.noZaiHistory":
      "No Z.ai weekly history yet; current subscription quotas from cache are still shown above.",
    "compare.allPeriods": "All periods",
    "compare.thisWeek": "This week",
    "compare.ongoing": "~ in progress",
    "compare.zaiWeeklyQuota": "Z.ai weekly quota",
    "compare.currentUsed": "Currently used {pct}%",
    "compare.periodEndUsed": "Used {pct}% at period end",
    "compare.currentUsedLabel": "Currently used %",
    "compare.periodEndUsedLabel": "Used % at end",
    "compare.startUsed": "Used % at start",
    "compare.peakUsed": "Observed peak %",
    "compare.periodPercentHint":
      "Start, peak, and current/end are all the used percentage of Z.ai's account-level weekly quota, not an Agent token percentage.",
    "compare.tokensOfAgents": "Agent actual token count (not a quota percentage)",
    "compare.tokenShort": "tokens",
    "compare.endUsedShort": "{pct}% used",
    "compare.requestsCount": "{count} requests",
    "compare.noUsage": "No enabled-agent usage in this period",
    "compare.progressLabel": "Used percentage at period end",
    "compare.samples": "{count} samples",
    "compare.samplesLow": " · sparse",
    "compare.footer":
      "Percentage = used share of the Z.ai account-level weekly quota; tokens = actual usage from enabled agents under the current filter. They are different metrics.",
  } satisfies typeof compare),

  ...({
    "report.title": "Usage Report",
    "report.refresh": "Refresh report",
    "report.today": "Today",
    "report.last7": "Last 7 days",
    "report.allDevices": "All devices",
    "report.loading": "Building report…",
    "report.emptyTitle": "No usage in this range",
    "report.emptyHint":
      "Make sure agents are enabled and had requests today or in the last 7 days.",
    "report.refreshing": "Refreshing latest data…",

    "report.cnyHint": "CNY converted",
    "report.usdHint": "USD native",
    "report.tokenHint": "Visible agents",
    "report.requestsHint": "API calls",
    "report.activeAgents": "Active agents",
    "report.agentsHintOne": "Only 1 source in range",
    "report.agentsHint": "Sources with usage in range",

    "report.noTrend": "No trend data to plot",
    "report.agentDist": "Agent breakdown",
    "report.byCost": "By cost share",
    "report.byToken": "Unpriced; by token share",
    "report.modelRank": "Top models",
    "report.noModels": "No model breakdown available.",

    "report.conclusion": "Takeaways",
    "report.mainModel": "Main model: ",
    "report.mainModelLine": "{model} ({agent}), {tokens} tokens.",
    "report.peakWindow": "Peak window: ",
    "report.peakWindowLine": "{label}, {value}.",
    "report.unpricedWarn":
      "{tokens} tokens are unpriced, so total cost is understated; add model prices in Pricing.",
    "report.allPriced": "All used models are priced; costs are comparable across runs.",

    "report.quotaSnapshot": "Agent quota snapshot",
    "report.quotaScope": "Enabled agents with data",
    "report.accountLevel": "Account-level",
    "report.localRealtime": "Local realtime",
    "report.quotaSourceNote":
      "Z.ai quota comes from history snapshots; Codex, Claude and Cursor read local realtime APIs.",
    "report.resetUnknown": "Reset time unknown",
    "report.resetDays": "Resets in ~{n}d",
    "report.resetHours": "Resets in ~{n}h",

    "report.q.weeklyCurrent": "This week",
    "report.q.hour5Current": "Past 5h",
    "report.q.weeklyPeak": "Weekly peak",
    "report.q.hour5Peak": "5h peak",
    "report.q.mcp": "MCP",
    "report.q.hour5": "5h",
    "report.q.weekly": "Week",
    "report.q.auto": "Auto",
    "report.q.api": "API",
    "report.q.plan": "Plan",
    "report.q.onDemand": "On-demand",

    "report.markdownPreview": "Markdown preview",
    "report.viewMarkdown": "View Markdown",
    "report.hideMarkdown": "Hide Markdown",
    "report.copy": "Copy",
    "report.copied": "Copied ✓",
    "report.savedOpened": "Saved and opened ✓",

    "report.noteCursorDaily":
      "Cursor's official usage is daily, so the hourly trend excludes it; agent totals still include it.",

    "report.file.daily": "Daily-",
    "report.file.weekly": "Weekly-",

    "report.md.daily": "Daily Report",
    "report.md.weekly": "Weekly Report",
    "report.md.noData": "(no data)",
    "report.md.summaryLine": "Total cost {cost} | Tokens {tokens} | Requests {requests}",
    "report.md.agentLine": "{label}: {cost} | {tokens} tokens | {requests} requests",
    "report.md.top5": "Top 5 models",
    "report.md.modelLine": "{agent} / {model}: {cost} | {tokens} tokens",
    "report.md.tokenPeak": "Token peak: {label}, {tokens} tokens",
    "report.md.quotaSnapshot": "Quota snapshot",
    "report.md.quotaLine": "{label} ({scope}): [{windows}]",
    "report.md.quotaReset": " | {text}",
    "report.md.notes": "Notes",
    "report.md.footer": "Generated by ZBar · {date}",
  } satisfies typeof report),

  ...({
    "settings.title": "Settings",

    "settings.panelOpacity": "Panel opacity",
    "settings.opacity": "Opacity",
    "settings.opacityHint":
      "Lower values make the frosted panel more translucent. On dark theme keep it above 60% so text stays readable.",
    "settings.language": "Language",
    "settings.langZh": "简体中文",
    "settings.langEn": "English",

    "settings.autostart": "Launch at login",
    "settings.enable": "Enable",
    "settings.applying": "Applying…",
    "settings.autostartHint":
      "Start ZBar automatically after signing in to Windows or macOS. The panel stays hidden by default; open it from the tray.",
    "settings.readingState": "Reading status…",
    "settings.autostartReadFail": "Failed to read autostart state: {msg}",
    "settings.autostartFailOn": "Failed to enable autostart: {msg}",
    "settings.autostartFailOff": "Failed to disable autostart: {msg}",

    "settings.sources": "Stats sources",
    "settings.instant": "Instant",
    "settings.sourcesHint":
      "Hiding a source only removes its tab and summary card; local collection and device sync are unaffected.",
    "settings.agentZaiDesc": "Z.ai usage & Coding Plan quota",
    "settings.agentCodexDesc": "Codex CLI usage & quotas",
    "settings.agentClaudeDesc": "Claude Code usage & plan quotas",
    "settings.agentCursorDesc": "Cursor usage & plan quotas",

    "settings.fxCard": "Exchange rate",
    "settings.fxAutoNote": "Auto-updating; uncheck to edit manually",
    "settings.fxNever": "Never fetched",
    "settings.fxUnknownSource": "unknown source",
    "settings.fxDailyTitle": "Refresh the rate online once a day",
    "settings.fxDaily": "Update rate daily",
    "settings.updateNow": "Update now",
    "settings.updating": "Updating…",
    "settings.updateNowTitle": "Fetch the latest rate online (multiple free sources with fallback)",
    "settings.fxNote":
      "Prices are stored in USD; CNY costs are converted at this rate (same source as the ¥ view in Pricing).",

    "settings.shortcut": "Global shortcut",
    "settings.shortcutHint":
      "Show/hide the panel. Format like alt+shift+z (modifiers: ctrl/alt/shift/cmd; main key: a letter or number).",
    "settings.apply": "Apply",
    "settings.applied": "Applied ✓",
  } satisfies typeof settings),
};

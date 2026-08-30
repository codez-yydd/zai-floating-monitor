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
import { theme } from "./dicts/theme";
import { projects } from "./dicts/projects";
import { share } from "./dicts/share";
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
    "common.delete": "Delete",
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
    "common.resetAt": "Resets at {time}",

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

    // Speed / current model
    "common.avgSpeed": "Avg speed",
    "common.fastest": "Fastest",
    "common.ttft": "First token",
    "common.currentModel": "Current model",
    "common.justNow": "just now",
    "common.minutesAgo": "{n}m ago",
    "common.hoursAgo": "{n}h ago",
    "common.daysAgo": "{n}d ago",

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
    "stats.reports": "Reports",
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
    "quota.allAccounts": "All accounts",
    "quota.quotaFail": "Quota query failed",
    "quota.weekShort": "Wk",
    "quota.hour5Short": "5h",

    // Agent 用量面板
    "stats.rateLimits": "Quotas",
    "stats.noDataFor": "No {name} data",
    "stats.codexNotFound": "No Codex detected",
    "stats.codexNotFoundHint":
      "Install and run OpenAI Codex CLI to create local sessions\n(~/.codex/sessions), then check back",
    "stats.claudeNotFound": "No Claude Code detected",
    "stats.claudeNotFoundHint":
      "Install and run Claude Code to create local sessions\n(~/.claude/projects), then check back",
    "stats.kimiNotFound": "No Kimi Code detected",
    "stats.kimiNotFoundHint":
      "Install and run Kimi Code CLI to create local sessions\n(~/.kimi-code/sessions), then check back",
    "stats.boosterBalance": "Booster balance",
    "stats.boosterMonthlyUsed": "Monthly used ¥{amount}",
    "stats.kimiMonthlyQuota": "Monthly quota",

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
    "compare.emptyTitle": "No weekly quota data yet",
    "compare.emptyHint":
      "Weekly quotas are sampled automatically while the app runs.\nKeep it running to accumulate data.",
    "compare.chartTitle": "Weekly quota used by subscription",
    "compare.chartHint":
      "Bar height = peak used % of each subscription's weekly quota within a calendar week.",
    "compare.allPeriods": "All weeks",
    "compare.thisWeek": "This week",
    "compare.selectedWeek": "Selected week",
    "compare.weekRange": "{from} ~ {to}",
    "compare.seriesHint":
      "Peak used % of each subscription in this week; \"No data\" = no samples this week",
    "compare.peakShort": "Peak",
    "compare.noDataShort": "No data",
    "compare.peakUsedPct": "peak {pct}% in week",
    "compare.noSample": "no samples this week",
    "compare.legacyAccount": "legacy data",
    "compare.legacyAccountHint":
      "Snapshots recorded before per-account tracking; cannot be attributed to a specific account",
    "compare.samples": "{count} samples",
    "compare.tokensOfAgents": "Agent actual token count (not a quota percentage)",
    "compare.tokenShort": "tokens",
    "compare.requestsCount": "{count} requests",
    "compare.noUsage": "No usage from enabled agents in this week",
    "compare.footer":
      "Quota % = peak used percentage of each subscription's weekly quota window within a calendar week; tokens = actual usage from enabled agents in that week. They are different metrics.",
  } satisfies typeof compare),

  ...({
    "report.title": "Usage Report",
    "report.refresh": "Refresh report",
    // Range selection reuses the shared RangePicker (range.* dict)
    "report.allDevices": "All devices",
    "report.loading": "Building report…",
    "report.emptyTitle": "No usage in this range",
    "report.emptyHint":
      "Make sure agents are enabled and had requests within the selected range.",
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
    // Short forms for countdown + exact time combined display
    "report.resetInDays": "in ~{n}d",
    "report.resetInHours": "in ~{n}h",

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

    "report.copy": "Copy",
    "report.copied": "Copied ✓",
    "report.savedOpened": "Saved and opened ✓",

    "report.noteCursorDaily":
      "Cursor's official usage is daily, so the hourly trend excludes it; agent totals still include it.",

    "report.file.daily": "Daily-",
    "report.file.custom": "Report-",

    "report.md.daily": "Daily Report",
    "report.md.custom": "Usage Report",
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
    "settings.agentKimiDesc": "Kimi Code CLI usage & subscription quota",

    "settings.resetDisplay": "Reset time display",
    "settings.resetCountdown": "Countdown",
    "settings.resetDatetime": "Exact time",
    "settings.resetDisplayHint":
      "How quota reset time is shown; both can be on at once. Exact times use MM-DD HH:mm.",

    // Desktop pet size level names (shared by PetSizeLevelPicker in petStyles;
    // pet settings themselves now live in the skin page pet card only)
    "settings.petSizeLevel1": "Small",
    "settings.petSizeLevel2": "Medium",
    "settings.petSizeLevel3": "Default",
    "settings.petSizeLevel4": "Large",
    "settings.petSizeLevel5": "XL",

    "settings.fontSize": "Font size",
    "settings.fontSmall": "Small",
    "settings.fontStandard": "Default",
    "settings.fontLarge": "Large",
    "settings.fontXl": "Larger",
    "settings.fontSizeHint":
      "Scales all panel content proportionally. If larger fonts show less, drag the window bigger to see more.",
    "settings.winSize": "Window size",
    "settings.winSizeSmall": "Small",
    "settings.winSizeStandard": "Standard",
    "settings.winSizeLarge": "Large",
    "settings.winSizeXl": "XL",
    "settings.winSizeCustom": "Custom",
    "settings.winSizeHint":
      "Drag the panel edges to resize freely, or pick a preset; your size is remembered.",

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

    "settings.aboutCard": "About & update",
    "settings.currentVersion": "Version",
    "settings.checkUpdate": "Check update",
    "settings.checking": "Checking…",
    "settings.upToDate": "Up to date",
    "settings.newVersion": "New version {v}",
    "settings.downloadInstall": "Download & install",
    "settings.downloading": "Downloading {pct}%",
    "settings.installing": "Installing…",
    "settings.restartUpdate": "Restart to update",
    "settings.updateReady": "Version {v} is downloaded — restart to apply",
    "settings.backgroundDownloading": "Downloading update in the background…",
    "settings.updateFailed": "Update failed: {msg}",
    "settings.openDownloadPage": "Open download page",
    "settings.updateHint":
      "Checks for updates every hour and downloads in the background; a red dot appears on Settings when ready. Updates are signature-verified. The Chinese interface prefers the Gitee source and the English interface prefers GitHub, falling back to the other automatically on failure.",

    "settings.accountsCard": "ZCode account switch",
    "settings.accountsHint":
      "Capture the current ZCode login as a snapshot, then switch between accounts with one click (ZCode quits and relaunches automatically; the current login is backed up first and restored on failure). Snapshots stay local only and are never synced.",
    "settings.accountsCurrent": "Current",
    "settings.accountsUnknown": "Unrecognized",
    "settings.accountsCapture": "Capture login",
    "settings.accountsCapturing": "Capturing…",
    "settings.accountsCapturedNew": "New account snapshot saved ✓",
    "settings.accountsCapturedUpdate": "Account snapshot updated ✓",
    "settings.accountsSwitch": "Switch",
    "settings.accountsSwitching": "Switching…",
    "settings.accountsSwitched": "Switched to \"{name}\" ✓",
    "settings.accountsRelaunchFail":
      " (ZCode failed to relaunch automatically, please open it manually)",
    "settings.accountsConfirmTitle": "Switch account",
    "settings.accountsConfirmDesc":
      "This quits and relaunches the ZCode desktop app and switches the login to \"{name}\". The current login is backed up first and rolled back automatically on failure.",
    "settings.accountsRemoveConfirm":
      "Only the snapshot saved by this app is deleted; the current ZCode login is not affected.",
    "settings.accountsRename": "Rename",
    "settings.accountsRenameDesc": "Set a recognizable name for this snapshot.",
    "settings.accountsEmpty":
      "No snapshots yet. Log in to an account in the ZCode client and click \"Capture login\", then log in to another account and capture again — after that you can switch with one click.",
    "settings.accountsLoadFail": "Failed to load accounts: {msg}",

    "settings.accountsAutoSwitch": "Smart",
    "settings.accountsAutoConfirmTitle": "Smart-switch to \"{name}\"",
    "settings.accountsAutoSummary": "5h {h5}% left · Wk {wk}% left",
    "settings.accountsAutoReasonReady":
      "Both 5h and weekly quotas available, with the best combined headroom",
    "settings.accountsAutoReasonWait":
      "All accounts' 5h quotas are exhausted; this account's 5h quota resets first (in ~{min} min)",
    "settings.accountsAutoReasonWaitNoTime":
      "All accounts' 5h quotas are exhausted; picked the account whose 5h quota resets sooner",
    "settings.accountsAutoNoTarget":
      "No suitable account to switch to: other accounts failed to query or their weekly quotas are exhausted",
    "settings.accountsAutoSingle": "Only one account — no other account to switch to",
    "settings.accountsAutoNoData": "Quota data not ready yet, please retry later",
    "settings.accountsAutoToggle": "Auto-switch when 5h quota is exhausted",
    "settings.accountsAutoToggleHint":
      "When the current account's 5h quota runs out, automatically switch to an available account and relaunch ZCode. Interrupted tasks must be resumed manually from the ZCode task list.",
    "settings.accountsAutoNotifyOk":
      "Auto-switched to \"{name}\" (5h quota exhausted). Resume interrupted tasks from the ZCode task list.",
    "settings.accountsAutoNotifyFail":
      "5h quota exhausted, but no available account to switch to",
    "settings.accountsAutoLogOk": "{time} auto-switched to \"{name}\"",
    "settings.accountsAutoLogFail": "{time} auto-switch failed: no available account",
    "settings.accountsAutoLinuxUnsupported":
      "Account switching is not supported on this platform",
  } satisfies typeof settings),

  ...({
    "theme.title": "Live Wallpaper",
    "theme.toolbarEntry": "Live wallpaper",
    "theme.loading": "Loading wallpaper state…",

    "theme.cardTitle": "ZCode Live Wallpaper",
    "theme.cardHint":
      "Inject a live video wallpaper into the ZCode desktop app as the chat background. The original app is backed up before installing and can be restored anytime.",
    "theme.version": "Version {v}",

    "theme.statusInstalled": "Installed",
    "theme.statusNotInstalled": "Not installed",
    "theme.statusNeedsReinstall": "Reinstall needed",

    "theme.install": "Install live wallpaper",
    "theme.reinstall": "Reinstall",
    "theme.uninstall": "Restore original",

    "theme.confirmInstallTitle": "Install live wallpaper",
    "theme.confirmInstallDesc":
      "About to inject a live wallpaper into the ZCode desktop app:\n· ZCode app files will be modified; the first time, allow ZBar to modify ZCode in the system prompt (an administrator password is usually not required; only certain install locations may ask for one)\n· The original app is backed up automatically first and can be restored anytime\n· ZCode updates invalidate the theme and require a reinstall\nMake sure you understand the risks before continuing.",
    "theme.confirmUninstallTitle": "Restore original",
    "theme.confirmUninstallDesc":
      "ZCode will be replaced with the backup taken before installation; the live wallpaper and its parameters will be removed, restoring the official original.\nThis action may also show the system confirmation prompt (allow ZBar to modify ZCode).",
    "theme.confirmQuitNote":
      "ZCode is running and will quit automatically — save your conversations first.",
    // macOS-only update impact note in the confirm dialog (not shown on Windows)
    "theme.confirmMacUpdateNote":
      "Note: On macOS, after modifying the app, ZCode's built-in updater will no longer work (restoring the original won't bring it back). To update ZCode, please re-download it from the official website:",
    "theme.zcodeOfficialSite": "ZCode official site (zcode.z.ai)",

    "theme.needsReinstallBanner":
      "The skin needs to be reinstalled to keep working (ZCode was upgraded, or the install predates the pet feature — see the note below); reinstalling restores everything.",
    "theme.nodeMissingBanner":
      "Node.js was not found — injection depends on it. Install Node.js first (run `brew install node`, or download from nodejs.org), then retry.",
    "theme.backupMissingBanner":
      "The original backup is missing, so restore is unavailable; reinstall ZCode to recover.",

    "theme.installing": "Installing live wallpaper",
    "theme.uninstalling": "Restoring original",
    "theme.stage.precheck": "Prechecking",
    "theme.stage.quit": "Quitting app",
    "theme.stage.extract": "Extracting",
    "theme.stage.inject": "Injecting theme",
    "theme.stage.pack": "Repacking",
    "theme.stage.verify": "Verifying",
    "theme.stage.backup": "Backing up original",
    "theme.stage.replace": "Replacing files",
    "theme.stage.sign": "Re-signing",
    "theme.stage.launch": "Launch check",
    "theme.stage.cleanup": "Cleaning up",
    "theme.stage.done": "Done",
    "theme.stage.error": "Failed",

    "theme.paramsTitle": "Effects",
    "theme.paramsHint":
      "Saved as you drag and applied live within ~1s — no restart needed. Chat, sidebar and right-panel sliders work independently; all other areas keep a fixed ambient opacity.",
    "theme.paramWpBrightness": "Wallpaper brightness",
    "theme.paramWpSaturate": "Saturation",
    "theme.paramWpBlur": "Background blur",
    "theme.paramBaseAlpha": "Ambient tint",
    "theme.paramBaseAlphaHint":
      "Adds a semi-transparent theme-colored layer over the wallpaper to improve overall text readability; 50%+ recommended for light wallpapers.",
    "theme.paramMaskStrength": "Mask strength",
    "theme.paramTextShadow": "Text outline",
    "theme.paramTextShadowHint":
      "Adds a soft outline around UI text for better contrast; raise it for dark themes on light wallpapers, 0 disables it.",
    "theme.paramPanelOpacity": "Chat opacity",
    "theme.paramSidebarOpacity": "Sidebar opacity",
    "theme.paramSidebarRightOpacity": "Right panel opacity",
    "theme.paramPlaybackRate": "Playback speed",
    // Usage bar section (standalone card: font size + opacity + per-turn /
    // session total toggles, live reload)
    "theme.usageTitle": "Usage Bar",
    "theme.usageHint":
      "Look of the per-turn usage line (↑ input ↓ output · × requests · speed) at the end of each ZCode chat turn. Sliders save as you drag and apply live within ~1s.",
    "theme.paramUsageFontSize": "Font size",
    "theme.paramUsageFontSizeHint": "Text size of the usage line (9–16px)",
    "theme.paramUsageOpacity": "Opacity",
    "theme.paramUsageOpacityHint":
      "Text opacity of the usage line; the waiting state of an in-progress turn is fainter",
    // Session total bar toggle (usage.js V5: pinned above the chat input)
    "theme.usageSessionBar": "Session total bar",
    "theme.usageSessionBarHint":
      "Pins the running session total (total tokens plus the input/output/cache read/request breakdown) above the chat input, with live output speed and estimated growth while streaming",
    // Per-turn usage bar toggle (usage.js V19: usage line under each turn)
    "theme.usageTurnBar": "Per-turn usage bar",
    "theme.usageTurnBarHint":
      "Shows each turn's usage at the end of every chat turn (↑ input ↓ output · ⟲ cache read · × requests · speed · TTFT); font size and opacity follow the sliders above",
    // Symbol legend (mirrors the usage.js line format)
    "theme.usageLegend":
      "↑ input (non-cached) · ↓ output · ⟲ cache read · × model requests · t/s output speed · TTFT time to first token · Σ session total tokens (input+output+cache read) · ≈ in-flight output estimate (excluded from totals)",

    // Desktop pet section (the single entry for pet settings: master toggle
    // + injected/floating mode + style/size, persisted in pet.json, applies
    // instantly; injected-pet parameters apply within ~1s via variables.css
    // hot reload)
    "theme.petTitle": "Desktop Pet",
    "theme.petHint":
      "Keeps a pixel pet that animates with ZCode's live work state: by default it lives inside the ZCode chat page (requires the skin installed, draggable), or switch it to a standalone floating window; style and size apply to both modes and take effect immediately.",
    "theme.petEnabled": "Enable desktop pet",
    "theme.petEnabledHint":
      "Master switch: shows the pet in the mode selected below; off hides it entirely",
    "theme.petModeLabel": "Mode",
    "theme.petModeInjected": "Built-in",
    "theme.petModeFloating": "Floating window",
    "theme.petModeHint":
      "Built-in: rendered inside the ZCode chat page, follows the skin's hot reload (~1s) and can be dragged anywhere; requires the live wallpaper skin to be installed. Floating window: a standalone transparent always-on-top pet, independent of the skin, also draggable.",
    "theme.petApplyFail": "Failed to apply pet settings: {msg}",
    "theme.petLoadFail": "Failed to load pet settings: {msg}",
    // Custom pets (phase 3: Petdex-format import, PetStyleSection in petStyles;
    // since V8 the built-in group is the bundled Zhipu girl sheet pet, whose
    // name comes from the list data's meta.name)
    "theme.petGroupBuiltin": "Built-in styles",
    "theme.petGroupCustom": "Custom pets (Petdex)",
    "theme.petBuiltinLoading": "Loading built-in pet…",
    "theme.petCustomEmpty":
      "No custom pets yet. Drop a Petdex pet package onto the window to import one.",
    // Skin page variant: png/webp drops route to wallpaper import once the
    // skin is installed; before that they route to pet import
    "theme.petImportHintSkin":
      "Import a custom pet: drop a Petdex package onto the window (zip / pet.json with its spritesheet alongside; pack bare png/webp sheets into a zip first — or drop them directly before the skin is installed)",
    "theme.petImporting": "Importing pet…",
    "theme.petImportDone": "Pet imported ✓ Tap it in the pet card to use",
    "theme.petImportFail": "Failed to import pet: {msg}",
    "theme.petDelete": "Delete",
    "theme.petDeleteConfirm":
      "Delete custom pet \"{name}\"? If it is currently in use, the built-in style will be restored.",
    "theme.petDeleteFail": "Failed to delete pet: {msg}",
    "theme.paramPetSize": "Pet size",
    "theme.paramPetSizeHint": "Sized by screen-height ratio (about 5.5%–15%); adapts when you switch screens or change resolution",
    // Pet state legend (mirrors the nine states of the pet.js state machine;
    // V9 splits thinking/walking)
    "theme.petLegend":
      "States: sleeping (ZCode idle for over 1 minute or ZBar not running) · idle (recent turn activity) · thinking (model planning, no output yet) · typing (output growing; speed tiers by token rate) · tool running (a tool is executing, e.g. command/build) · walking (heading to a new turn / waiting for the next) · celebrating (turn finished, about 3s) · failed (turn failed or cancelled, about 3s)",
    "theme.currentWallpaper": "Current wallpaper",
    "theme.noWallpaper": "No wallpaper selected",
    "theme.lightWallpaperPreset": "Light wallpaper fit",
    "theme.lightWallpaperPresetDesc":
      "One click to apply the recommended values for light wallpapers (ambient tint 55%, mask 35%, brightness 60%, text outline 60%).",
    "theme.lightWallpaperPresetFail": "Failed to apply light wallpaper preset: {msg}",
    "theme.resetParams": "Reset to defaults",
    "theme.paramsSavedFlash": "Saved ✓",

    "theme.libraryTitle": "Wallpaper Library",
    "theme.libraryHint":
      "Click a wallpaper to apply it instantly (live within ~1s). Videos and images supported.",
    "theme.libraryDirLabel": "Wallpaper folder",
    "theme.libraryDirEmpty":
      "Not set. Drop a folder onto the zone below to set it as the wallpaper folder.",
    "theme.libraryClearDir": "Clear folder",
    "theme.libraryEmpty":
      "No wallpapers yet. Drop video / image files to import, or set a wallpaper folder to scan automatically.",
    "theme.libraryLoading": "Loading wallpapers…",
    "theme.libraryKindVideo": "Video",
    "theme.libraryKindImage": "Image",
    "theme.defaultWallpaperName": "Default Aurora",
    "theme.selecting": "Switching…",
    // Preview card: placeholder when the media file fails to load
    "theme.previewUnavailable": "Preview unavailable",

    // Drop zone (wallpaper / folder entry: the native file dialog is invisible
    // on dockless Accessory apps)
    "theme.dropWallpaperHint":
      "Drop a video / image file to change the wallpaper (mp4 / webm / mov / jpg / png / webp), or drop a folder to set the wallpaper folder",
    "theme.dropWallpaperBusy": "Processing dropped item…",

    "theme.installDone": "Live wallpaper installed ✓",
    "theme.uninstallDone": "ZCode restored to original ✓",
    "theme.wallpaperSet": "Wallpaper set to {name} ✓",
    "theme.wallpaperDirSet": "Wallpaper folder set: {path} ✓",
    "theme.wallpaperDirCleared": "Wallpaper folder cleared ✓",

    "theme.loadStateFail": "Failed to load wallpaper state: {msg}",
    "theme.loadParamsFail": "Failed to load effects: {msg}",
    "theme.setParamsFail": "Failed to save parameters: {msg}",
    "theme.setWallpaperFail": "Failed to set wallpaper: {msg}",
    "theme.selectWallpaperFail": "Failed to switch wallpaper: {msg}",
    "theme.setWallpaperDirFail": "Failed to set wallpaper folder: {msg}",
    "theme.listWallpapersFail": "Failed to load wallpapers: {msg}",
    "theme.resetParamsFail": "Failed to reset parameters: {msg}",
    "theme.installFail": "Install failed: {msg}",
    "theme.uninstallFail": "Restore failed: {msg}",

    // Restart ZCode (full reload of injected theme assets)
    "theme.restartZcode": "Restart ZCode",
    "theme.restarting": "Restarting…",
    "theme.confirmRestartTitle": "Restart ZCode",
    "theme.confirmRestartDesc":
      "ZCode will quit and relaunch (takes a few seconds) so the injected live wallpaper assets fully reload. If ZCode isn't running, it will simply be launched.\nSave any conversations in progress first.",
    "theme.restartDone": "ZCode restarted — injected assets fully reloaded ✓",
    "theme.launchDone": "ZCode wasn't running — launched directly ✓",
    "theme.restartFail": "Failed to restart ZCode: {msg}",
  } satisfies typeof theme),

  ...({
    // Projects tab (project list + session drill-down)
    "projects.tab": "Projects",
    "projects.title": "Project usage",
    "projects.back": "Back to projects",

    "projects.unknown": "Unknown project",
    "projects.loadFail": "Failed to load projects: {msg}",
    "projects.empty": "No project data in this range",
    "projects.emptyHint":
      "Projects with sessions in the selected range will show up here.",
    "projects.agentMix": "Agent mix",
    "projects.sessionCount": "{count} sessions",

    "projects.sessionsOf": "Sessions of {name}",
    "projects.agentFilter": "Agent",
    "projects.all": "All",
    "projects.loadMore": "Load more ({loaded}/{total})",
    "projects.loadingMore": "Loading…",
    "projects.noMore": "All loaded",
    "projects.sessionsEmpty": "No sessions for this project in the selected range",
    "projects.sessionsLoadFail": "Failed to load sessions: {msg}",

    "projects.models": "Models",
    "projects.in": "In",
    "projects.out": "Out",
    "projects.cacheRead": "Cache read",
    "projects.cacheWrite": "Cache write",
    "projects.duration": "Duration",
    "projects.speed": "Speed",
    "projects.ttft": "First tok",
  } satisfies typeof projects),

  ...({
    "share.button": "Share card",
    "share.title": "Generate share card",
    "share.periodTitle": "This Week's AI Usage",
    "share.totalCost": "Total cost",
    "share.totalTokens": "Total tokens",
    "share.totalRequests": "Total requests",
    "share.topModels": "Top 5 models",
    "share.topProjects": "Top 5 projects",
    "share.heatTitle": "Last 7 days",
    "share.footer": "Generated by ZBar",
    "share.save": "Save image",
    "share.saving": "Saving…",
    "share.saved": "Saved ✓ {path}",
    "share.saveFail": "Save failed: {msg}",
    "share.close": "Close",
    "share.loading": "Generating…",
    "share.loadFail": "Failed to load data: {msg}",
    "share.regenerate": "Regenerate",
  } satisfies typeof share),
};

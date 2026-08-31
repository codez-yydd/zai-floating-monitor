/**
 * 凭证管理词典：通用凭证体系（CredentialsCard / GenericQuotaPanel）的
 * 空态引导、添加/编辑/删除、类型与区域徽章、校验状态等文案。
 * guide.<provider> 为各服务的获取凭证引导文案键位（值待各服务接入时细化）。
 */

export const credentials = {
  // ===== 卡片与空态 =====
  "credentials.cardTitle": "凭证",
  "credentials.emptyTitle": "还没有添加凭证",
  "credentials.add": "添加凭证",
  "credentials.edit": "编辑凭证",
  "credentials.countBadge": "{n} 条",

  // ===== 添加 / 编辑弹层 =====
  "credentials.label": "备注",
  "credentials.labelPlaceholder": "如：Pro 订阅",
  "credentials.labelHint": "32 字以内，便于区分多个凭证",
  "credentials.secret": "凭证内容",
  "credentials.secretPlaceholderApiKey": "粘贴 API Key",
  "credentials.secretPlaceholderCookie": "粘贴 Cookie 字符串",
  "credentials.secretPlaceholderToken": "粘贴访问 Token",
  "credentials.secretKeepHint": "留空表示不修改已保存的凭证内容",
  "credentials.secretHintCookie":
    "粘贴浏览器复制的 Cookie 请求头或整段 cURL 命令",
  "credentials.secretShow": "显示凭证内容",
  "credentials.secretHide": "隐藏凭证内容",
  "credentials.region": "区域",
  "credentials.regionNone": "默认",
  "credentials.regionCn": "国内站",
  "credentials.regionGlobal": "国际站",
  "credentials.secretRequired": "请输入凭证内容",
  "credentials.labelRequired": "备注不能为空",

  // ===== 类型徽章 =====
  "credentials.kindApiKey": "API Key",
  "credentials.kindCookie": "Cookie",
  "credentials.kindToken": "Token",

  // ===== 校验状态 / 时间 =====
  "credentials.checkOk": "校验通过",
  "credentials.checkFail": "校验失败",
  "credentials.notChecked": "未校验",
  "credentials.updatedAt": "更新于 {time}",

  // ===== 删除确认 / 反馈 =====
  "credentials.deleteTitle": "删除凭证",
  "credentials.deleteConfirm": "删除「{name}」后不可恢复，确认删除？",
  "credentials.saved": "已保存 ✓",
  "credentials.saveFail": "保存失败：{msg}",
  "credentials.loadFail": "读取凭证失败：{msg}",

  // ===== 凭证文件损坏自愈（列表读取失败时的重置入口，二次确认）=====
  "credentials.resetFile": "重置凭证文件",
  "credentials.resetTitle": "重置凭证文件",
  "credentials.resetConfirm":
    "将清除该服务全部已存凭证并重建空文件（用于修复损坏的凭证文件），已删除的凭证不可恢复，确认重置？",
  "credentials.resetDone": "已重置 ✓",
  "credentials.resetFail": "重置失败：{msg}",

  // ===== 添加服务入口（tab 栏「＋」按钮 / 服务选择浮层 / 设置页快捷入口）=====
  "credentials.addService": "添加服务",
  "credentials.addServiceTitle": "添加订阅服务",
  "credentials.addServiceHint": "选择服务并填入凭证，多个订阅可重复添加",
  "credentials.comingSoon": "查询即将上线",
  "credentials.entriesCount": "{n} 条凭证",

  // ===== 通用额度面板 =====
  "credentials.quotaPending":
    "额度查询接入即将上线。凭证已就绪，接入后将自动在此展示用量与额度。",
  "credentials.quotaRefreshing": "正在查询额度，请稍候…",
  "credentials.entryError": "查询失败",
  "credentials.entryExpired": "已过期",
  "credentials.entryPending": "待接入",
  "credentials.balance": "余额",
  "credentials.balanceGranted": "赠送 {amount}",
  "credentials.balanceToppedUp": "充值 {amount}",

  // ===== 额度窗口标题（QuotaEntryCard 按 window.key 映射；Rust 各 provider
  //      模块下发的 title 为硬编码中文，英文界面经此翻译，未收录的 key 回落
  //      Rust title）。zh 取各 provider 当前中文标题的最通用口径 =====
  "credentials.windowTitle.hour5": "5 小时",
  "credentials.windowTitle.weekly": "本周",
  "credentials.windowTitle.monthly": "本月",
  "credentials.windowTitle.interval": "当前窗口",
  "credentials.windowTitle.credits": "积分",
  "credentials.windowTitle.quota": "Token 配额",
  "credentials.windowTitle.fuel": "加油包",
  "credentials.windowTitle.sub_credits": "订阅积分",
  "credentials.windowTitle.topup_credits": "充值积分",
  "credentials.windowTitle.opus_weekly": "Opus 周额度",
  "credentials.windowTitle.sonnet_weekly": "Sonnet 周额度",
  "credentials.windowTitle.extra_usage": "超额消费",
  "credentials.windowTitle.pro": "Pro 模型",
  "credentials.windowTitle.flash": "Flash 模型",
  "credentials.windowTitle.auto": "Auto",
  "credentials.windowTitle.api": "API",

  // ===== 本地型 provider 的凭证可选引导（数据来自本地 CLI 登录态/数据库，凭证非必需）=====
  "credentials.localEmptyTitle": "本地数据源，无需凭证",
  "credentials.localEmptyHint":
    "该服务自动读取本地 CLI 登录态，无需凭证；如需也可手动添加。",

  // ===== 各服务获取凭证的引导文案（键位预留，接入时按控制台路径细化）=====
  // guideBrief.<provider> 为空态首句（去哪登录/获取）；完整 guide.<provider>
  //（含 region 一致性、所需 cookie 项等前提条件）经「查看接入步骤」展开显示
  "credentials.guideMore": "查看接入步骤",
  "credentials.guideLess": "收起步骤",
  "credentials.guideBrief.claude":
    "从本机 ~/.claude/.credentials.json 复制 sk-ant-oat 开头的 OAuth Access Token 粘贴保存。",
  "credentials.guideBrief.cursor":
    "登录 cursor.com 后复制 Cookie 粘贴保存，或自动读取本机已登录 Cursor 的本地登录态。",
  "credentials.guideBrief.grok":
    "安装 Grok CLI 并运行 grok login 即可自动读取本地登录态；也可从 ~/.grok/auth.json 复制 key 作为 Token 手动添加。",
  "credentials.guideBrief.qoder":
    "登录 Qoder（国际站 qoder.com / 中国站 qoder.com.cn）。",
  "credentials.guideBrief.minimax":
    "在 MiniMax 开放平台的「API Keys」页创建 Coding Plan 的 API Key。",
  "credentials.guideBrief.moonshot":
    "在 Moonshot 开放平台的「API Key 管理页」创建 API Key。",
  "credentials.guideBrief.deepseek":
    "在 DeepSeek 开放平台（platform.deepseek.com）的「API Keys」页创建 API Key。",
  "credentials.guideBrief.longcat": "登录 longcat.chat，打开 platform/usage 用量页。",
  "credentials.guideBrief.mimo":
    "登录小米 MiMo 开放平台（platform.xiaomimimo.com）。",
  "credentials.guideBrief.alibaba":
    "在阿里云百炼控制台的「API-KEY 管理」页创建 DashScope API Key 并订阅 Coding Plan。",
  "credentials.guideBrief.alibabatoken":
    "登录阿里云百炼控制台订阅页（中国站 / 国际站）。",
  "credentials.guideBrief.stepfun":
    "登录阶跃星辰开放平台（platform.stepfun.com）。",
  "credentials.guideBrief.doubao": "在火山引擎（火山方舟）获取 API Key 后粘贴保存。",
  "credentials.guideBrief.kimi":
    "推荐使用 OAuth 网页登录（凭证自动保存）；也可手动粘贴 OAuth 令牌（refresh_token）或 API Key。",
  "credentials.guide.kimi":
    "点击「网页登录」按提示在浏览器完成 Kimi 授权，凭证将以 refresh_token 形态自动保存（无需安装 Kimi Code CLI）；也可在「凭证类型」切换为 API Key 后粘贴保存。区域需与账号所属站点一致（国际站账号请选国际站，网页登录同理），成功后自动展示该账号的 5 小时/本周/本月额度与加油包余额。",
  // ===== Kimi OAuth 网页登录（设备码流程）=====
  "credentials.oauthEntryTitle": "网页登录（推荐）",
  "credentials.oauthEntryButton": "网页登录",
  "credentials.oauthEntryHint":
    "在浏览器完成 Kimi 授权，凭证自动保存，无需手动粘贴 Token；国际站账号请先在下方选择「国际站」区域",
  "credentials.oauthTitle": "Kimi 网页登录",
  "credentials.oauthStepHint":
    "1. 点击「打开授权页面」在浏览器完成登录授权；2. 若页面要求输入，请粘贴下方确认码。",
  "credentials.oauthRegionCurrent": "当前区域：{region}",
  "credentials.oauthCopy": "复制确认码",
  "credentials.oauthCopied": "已复制",
  "credentials.oauthCopyFail": "复制失败，请手动复制",
  "credentials.oauthStarting": "正在发起登录…",
  "credentials.oauthWaiting": "等待授权中，请在新打开的页面完成确认…",
  "credentials.oauthValidFor": "（确认码 {minutes} 分钟内有效）",
  "credentials.oauthSuccess": "登录成功，凭证已保存",
  "credentials.oauthDenied": "已拒绝授权，如需登录请重新发起",
  "credentials.oauthExpired": "授权码已过期，请重新发起",
  "credentials.oauthBack": "返回手动填写",
  "credentials.oauthOpen": "打开授权页面",
  "credentials.oauthRetry": "重新发起",
  "credentials.oauthDone": "网页登录成功，凭证已保存",
  // ===== Kimi 凭证类型切换（OAuth 令牌 / API Key）=====
  "credentials.kindLabel": "凭证类型",
  "credentials.kindOAuthToken": "OAuth 令牌",
  "credentials.secretPlaceholderOauthToken": "粘贴 OAuth 令牌（refresh_token）",
  "credentials.guide.claude":
    "从本机 ~/.claude/.credentials.json（或另一台已登录该账号机器的同路径文件）复制 claudeAiOauth.accessToken（sk-ant-oat 开头的 OAuth Access Token）粘贴保存，即可在此查看该账号的订阅额度。",
  "credentials.guide.cursor":
    "登录 cursor.com → F12 Network 面板复制任意请求的 Cookie 请求头（WorkosCursorSessionToken 开头）→ 粘贴保存，将自动展示该账号的套餐额度；本机已登录 Cursor 应用时主面板会自动读取本地登录态，无需添加凭证。",
  "credentials.guide.gemini":
    "安装 Gemini CLI 并在终端完成 gemini 登录后，自动读取本地登录态展示用量与额度，无需添加凭证。",
  "credentials.guide.grok":
    "安装 Grok CLI 并运行 grok login 后自动读取本地登录态展示本月额度；也可从 ~/.grok/auth.json 复制 key 作为 Token 手动添加。",
  "credentials.guide.qoder":
    "登录 Qoder（国际站 qoder.com / 中国站 qoder.com.cn，区域需与所添凭证一致）→ 进入控制台/账户用量页 → F12 Network 面板复制任意请求的 Cookie 请求头（或整段 Copy as cURL）→ 粘贴保存，将自动展示大模型积分用量。",
  "credentials.guide.opencodego":
    "安装 OpenCode 并登录 Go 计划后，自动从本地数据库（~/.local/share/opencode/）估算用量与额度，无需添加凭证。",
  "credentials.guide.minimax":
    "在 MiniMax 开放平台的「API Keys」页（platform.minimaxi.com，国际站为 minimax.io 控制台）创建 Coding Plan 的 API Key（sk-cp- 开头 Token）后粘贴保存，将自动展示当前窗口与本周用量；区域需与所填 Key 的获取站点一致（国际站 Key 请选国际站）。",
  "credentials.guide.moonshot":
    "在 Moonshot 开放平台的「API Key 管理页」（platform.moonshot.cn，国际站为 platform.moonshot.ai）创建 API Key 后粘贴保存，将自动展示账户余额与赠送/充值拆分；区域需与所填 Key 的获取站点一致（国际站 Key 请选国际站）。",
  "credentials.guide.deepseek":
    "在 DeepSeek 开放平台（platform.deepseek.com）的「API Keys」页创建 API Key 后粘贴保存，将自动展示账户余额与赠送/充值拆分。",
  "credentials.guide.longcat":
    "登录 longcat.chat → 打开 platform/usage 用量页 → F12 Network 面板复制任意请求的 Cookie 请求头（或整段 Copy as cURL）→ 粘贴保存，将自动展示 Token 配额与加油包剩余。",
  "credentials.guide.mimo":
    "登录小米 MiMo 开放平台（platform.xiaomimimo.com）→ F12 Network 面板复制任意请求的 Cookie 请求头（或整段 Copy as cURL，需含 api-platform_serviceToken 与 userId 两项）→ 粘贴保存，将自动展示账户余额与当前积分用量。",
  "credentials.guide.alibaba":
    "在阿里云百炼控制台的「API-KEY 管理」页（中国站 bailian.console.aliyun.com / 国际站 modelstudio.console.alibabacloud.com，区域需与所添凭证一致）创建 DashScope API Key 并订阅 Coding Plan（通义灵码）后粘贴保存，将自动展示 5 小时/本周/本月用量。",
  "credentials.guide.alibabatoken":
    "登录阿里云百炼控制台订阅页（中国站 bailian.console.aliyun.com / 国际站 modelstudio.console.alibabacloud.com，区域需与所添凭证一致）→ F12 Network 面板筛选 tokenplan 请求 → 复制该请求的 Cookie 请求头（或整段 Copy as cURL）→ 粘贴保存，将自动展示 Team 积分池或 Personal/Solo 的 5 小时/7 天滚动用量（Personal/Solo 需 quota 主机 bailian-cs.console.aliyun.com / bailian-singapore-cs.alibabacloud.com 域下请求的 Cookie）。",
  "credentials.guide.stepfun":
    "登录阶跃星辰开放平台（platform.stepfun.com）→ F12 Network 面板找任意 Dashboard 请求 → 复制 Cookie 里 Oasis-Token 的值粘贴保存，将自动展示 5 小时/本周用量或订阅/充值积分余量。",
  "credentials.guide.doubao":
    "在火山引擎（火山方舟）获取 API Key 后粘贴保存，额度查询接入后自动展示用量与额度。",
};

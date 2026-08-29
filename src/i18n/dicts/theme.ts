/**
 * 动态壁纸页词典：Agent 应用卡片（安装/还原/进度）与效果参数区的文案。
 * 进度 stage 文案（theme.stage.*）与 Rust 侧 zbar://agent-theme-progress
 * 事件的 stage 值一一对应。
 */

export const theme = {
  "theme.title": "动态壁纸",
  // 主面板工具栏入口按钮（🎨）
  "theme.toolbarEntry": "动态壁纸",
  "theme.loading": "正在读取壁纸状态…",

  // ===== Agent 应用卡片 =====
  "theme.cardTitle": "ZCode 动态壁纸",
  "theme.cardHint":
    "把动态视频注入 ZCode 桌面应用作为对话背景。安装前自动备份原版，可随时还原。",
  "theme.version": "版本 {v}",

  // 状态徽章
  "theme.statusInstalled": "已安装",
  "theme.statusNotInstalled": "未安装",
  "theme.statusNeedsReinstall": "需重装",

  // 操作按钮
  "theme.install": "安装动态壁纸",
  "theme.reinstall": "重新安装",
  "theme.uninstall": "还原原版",

  // 确认浮层
  "theme.confirmInstallTitle": "安装动态壁纸",
  "theme.confirmInstallDesc":
    "即将把动态壁纸注入 ZCode 桌面应用：\n· 会直接修改 ZCode 应用文件，首次需在系统弹窗中允许 ZBar 修改 ZCode（管理员密码通常不需要；仅特定安装位置会要求）\n· 安装前会自动备份原版，之后可随时还原\n· ZCode 升级后主题会失效，需重新安装\n建议先了解上述风险后再继续。",
  "theme.confirmUninstallTitle": "还原原版",
  "theme.confirmUninstallDesc":
    "将使用安装前的备份替换 ZCode 应用，动态壁纸与效果参数会被移除，ZCode 恢复为官方原版。\n此操作同样可能弹出系统确认框（允许 ZBar 修改 ZCode）。",
  // 目标应用运行中时拼在确认文案末尾（安装/还原共用）
  "theme.confirmQuitNote":
    "ZCode 正在运行，操作前会自动退出，请先保存进行中的对话。",
  // macOS 确认浮层的更新影响提示（仅 macOS 渲染，Windows 无此限制）
  "theme.confirmMacUpdateNote":
    "注意：macOS 上修改应用后，ZCode 的内置更新将不可用（还原原版也无法恢复）。需要更新 ZCode 时，请到官网重新下载安装：",
  "theme.zcodeOfficialSite": "ZCode 官网（zcode.z.ai）",

  // 状态横幅
  "theme.needsReinstallBanner":
    "检测到 ZCode 已升级，动态壁纸已失效，建议重新安装以恢复效果。",
  "theme.nodeMissingBanner":
    "未检测到 Node.js，注入依赖它完成。请先安装 Node.js（终端执行 brew install node，或到 nodejs.org 下载）后重试。",
  "theme.backupMissingBanner":
    "原版备份缺失，无法还原到未安装状态；如需恢复请重新安装 ZCode。",

  // ===== 安装/还原进度 =====
  "theme.installing": "正在安装动态壁纸",
  "theme.uninstalling": "正在还原原版",
  // stage 值 → 分阶段文案（与 Rust 侧事件契约一一对应）
  "theme.stage.precheck": "预检",
  "theme.stage.quit": "退出应用",
  "theme.stage.extract": "解包",
  "theme.stage.inject": "注入主题",
  "theme.stage.pack": "重新打包",
  "theme.stage.verify": "校验",
  "theme.stage.backup": "备份原版",
  "theme.stage.replace": "替换文件",
  "theme.stage.sign": "重签名",
  "theme.stage.launch": "启动验证",
  "theme.stage.cleanup": "清理",
  "theme.stage.done": "完成",
  "theme.stage.error": "失败",

  // ===== 效果参数区 =====
  "theme.paramsTitle": "效果参数",
  "theme.paramsHint":
    "拖动滑块即时保存并热生效（约 1 秒），无需重启 ZCode。对话区、侧栏与右栏滑块各自独立生效，界面上其余区域保持固定的氛围透明度。",
  "theme.paramWpBrightness": "壁纸亮度",
  "theme.paramWpSaturate": "饱和度",
  "theme.paramWpBlur": "背景模糊",
  "theme.paramBaseAlpha": "氛围底",
  "theme.paramBaseAlphaHint":
    "在壁纸之上垫一层主题色半透明底，提升全局文字可读性；亮色壁纸建议 50% 以上",
  "theme.paramMaskStrength": "遮罩浓度",
  "theme.paramTextShadow": "文字描边",
  "theme.paramTextShadowHint":
    "给界面文字补一圈柔和描边增强对比；亮壁纸配暗色主题时适当调高，0 为关闭",
  "theme.paramPanelOpacity": "对话区不透明度",
  "theme.paramSidebarOpacity": "侧栏不透明度",
  "theme.paramSidebarRightOpacity": "右栏不透明度",
  "theme.paramPlaybackRate": "播放速度",
  // 用量统计条区（独立配置区域：字号 + 不透明度 + 每轮统计条/会话累计条
  // 开关，热重载生效）
  "theme.usageTitle": "用量统计条",
  "theme.usageHint":
    "ZCode 对话内每轮末尾统计条（↑输入 ↓输出 · ×请求数 · 速度等）的外观，拖动滑块即时保存并热生效（约 1 秒）。",
  "theme.paramUsageFontSize": "字号",
  "theme.paramUsageFontSizeHint": "统计条文字大小（9~16px）",
  "theme.paramUsageOpacity": "不透明度",
  "theme.paramUsageOpacityHint":
    "统计条文字不透明度；进行中的轮次等待态会更淡一些",
  // 会话累计条开关（usage.js V5：固定悬浮于对话输入框上方的会话级实时统计条）
  "theme.usageSessionBar": "会话累计条",
  "theme.usageSessionBarHint":
    "在对话输入框上方固定显示当前会话累计（总 Token 与输入/输出/缓存读/请求数明细），流式生成时实时显示输出速度与估算增量",
  // 每轮统计条开关（usage.js V19：ZCode 对话内每轮回复末尾的用量统计行）
  "theme.usageTurnBar": "每轮统计条",
  "theme.usageTurnBarHint":
    "在每轮对话末尾显示该轮用量（↑ 输入 ↓ 输出 · ⟲ 缓存读 · × 请求数 · 速度 · TTFT），字号与不透明度沿用上方滑块",
  // 统计条符号图例（与 usage.js 行格式一一对应）
  "theme.usageLegend":
    "↑ 输入（非缓存） · ↓ 输出 · ⟲ 缓存读 · × 模型请求数 · t/s 输出速度 · TTFT 首字延迟 · Σ 会话总 Token（输入+输出+缓存读） · ≈ 生成中输出估算（未计入累计）",
  "theme.currentWallpaper": "当前壁纸",
  "theme.noWallpaper": "未选择壁纸",
  "theme.lightWallpaperPreset": "亮色壁纸适配",
  "theme.lightWallpaperPresetDesc":
    "一键调整为亮色壁纸推荐参数（氛围底 55%、遮罩 35%、壁纸亮度 60%、文字描边 60%）",
  "theme.lightWallpaperPresetFail": "亮色壁纸适配失败：{msg}",
  "theme.resetParams": "恢复默认参数",
  "theme.paramsSavedFlash": "参数已保存 ✓",

  // ===== 重启 ZCode =====
  // 注入的 theme.css / effects.js 依赖应用冷启动加载，手动改过注入文件等
  // 场景参数热重载覆盖不到，提供一键重启入口让注入资产完全重载
  "theme.restartZcode": "重启 ZCode",
  "theme.restarting": "重启中…",
  "theme.confirmRestartTitle": "重启 ZCode",
  "theme.confirmRestartDesc":
    "将退出并重新启动 ZCode 桌面应用（约需数秒），让注入的动态壁纸资产完全重载；ZCode 未在运行时会直接启动。\n请先保存 ZCode 中进行中的对话。",
  "theme.restartDone": "已重启 ZCode，注入资产已完全重载 ✓",
  "theme.launchDone": "ZCode 未在运行，已直接启动 ✓",
  "theme.restartFail": "重启 ZCode 失败：{msg}",

  // ===== 壁纸库 =====
  "theme.libraryTitle": "壁纸库",
  "theme.libraryHint":
    "点击壁纸即时切换（约 1 秒生效），支持视频与图片。",
  "theme.libraryDirLabel": "壁纸目录",
  "theme.libraryDirEmpty":
    "未设置。拖入一个文件夹到下方投放区即可设为壁纸目录。",
  "theme.libraryClearDir": "清除目录",
  "theme.libraryEmpty":
    "暂无壁纸。可拖入视频 / 图片文件导入，或设置壁纸目录后自动扫描。",
  "theme.libraryLoading": "正在读取壁纸列表…",
  "theme.libraryKindVideo": "视频",
  "theme.libraryKindImage": "图片",
  "theme.defaultWallpaperName": "默认流光",
  "theme.selecting": "切换中…",
  // 预览卡：视频/图片文件加载失败时的占位说明（文件缺失或格式不支持）
  "theme.previewUnavailable": "预览不可用",

  // 拖拽投放区（换壁纸 / 设壁纸目录入口：原生文件对话框在无 Dock 图标
  // 的 Accessory 应用上不可见，故接收 Tauri 拖放事件给出的路径）
  "theme.dropWallpaperHint":
    "拖入视频 / 图片文件换壁纸（mp4 / webm / mov / jpg / png / webp），或拖入文件夹设为壁纸目录",
  "theme.dropWallpaperBusy": "正在处理拖入内容…",

  // ===== 成功反馈 =====
  "theme.installDone": "动态壁纸安装完成 ✓",
  "theme.uninstallDone": "已还原 ZCode 原版 ✓",
  "theme.wallpaperSet": "壁纸已切换：{name} ✓",
  "theme.wallpaperDirSet": "壁纸目录已设置：{path} ✓",
  "theme.wallpaperDirCleared": "壁纸目录已清除 ✓",

  // ===== 错误提示 =====
  "theme.loadStateFail": "读取壁纸状态失败：{msg}",
  "theme.loadParamsFail": "读取效果参数失败：{msg}",
  "theme.setParamsFail": "保存参数失败：{msg}",
  "theme.setWallpaperFail": "设置壁纸失败：{msg}",
  "theme.selectWallpaperFail": "切换壁纸失败：{msg}",
  "theme.setWallpaperDirFail": "设置壁纸目录失败：{msg}",
  "theme.listWallpapersFail": "读取壁纸列表失败：{msg}",
  "theme.resetParamsFail": "恢复默认参数失败：{msg}",
  "theme.installFail": "安装失败：{msg}",
  "theme.uninstallFail": "还原失败：{msg}",
};

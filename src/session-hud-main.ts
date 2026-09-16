/**
 * 会话 Token 悬浮窗宿主壳（Session HUD）。
 *
 * 与宠物窗宿主壳（pet-main.ts）同款的零 React 薄壳：不引入 panel 的
 * 组件体系（widgets/layout 为 panel 专用），列表渲染在壳内直接完成。
 * 职责：
 * - 初始配置：invoke get_session_hud_config 读取（透明度 + 显示项 +
 *   活跃档位 + 字体缩放）；读取失败按 HTML/CSS 默认渲染（防御式，与
 *   pet 同风格）；
 * - 数据流：监听 zbar://session-hud-usage（session_hud.rs 轮询器每
 *   2 秒/速度字段 1 秒节拍推送的会话快照，内容变化才推），重建列表 DOM；
 * - 参数流：监听 zbar://session-hud-params（设置卡变更 / 滑块落盘回推），
 *   即时应用透明度、显示项与字体缩放，下一轮数据到达后列表按新显示项
 *   重建；
 * - 设置面板：header 右侧"设置"按钮在 header 下方弹出 #hud-settings 小
 *   弹层（再次点击 / 点击面板外区域 / Esc 收起），内含字体缩放（0.8~1.4，
 *   写 CSS 变量 --hud-scale，页面全部字号/行高 calc 随动，防抖经专用命令
 *   set_session_hud_font_scale 落盘 fontScale）与窗口透明度（25%~100%
 *   百分比刻度，存 opacity = 刻度/100，即时经 CSS opacity 生效、防抖经
 *   set_session_hud_opacity 落盘）两个细轨道滑块 + 数值读数；面板打开
 *   期间应用不透明度钳制不低于 0.5（读数可辨认，落盘值仍是用户刻度）；
 * - 关闭按钮：header 右侧"×"经 close_session_hud 把总开关置 false
 *   （停轮询 + 关窗，与设置页总开关关闭同一条路径），窗口随后销毁；与
 *   设置按钮之间留 6px 分隔且 hover 染警示色，压低误触代价；
 * - 拖动移动：#hud-header 为 data-tauri-drag-region 拖动区（标题/计数
 *   带同属性；按钮与滑块不带，且 mousedown/pointerdown stopPropagation
 *   双保险，"点按钮/拖滑块"不会拖动窗口；列表行不留拖动属性，避免后续
 *   行内 hover 交互误拖），位置由 Rust 侧 Moved 挂点节流持久化；
 * - 拖动调整尺寸：8 向隐形热区层由 hud-resize.ts 安装（undecorated
 *   窗口系统边缘热区在 Windows 上不生效，详见该模块文件头）；尺寸落盘
 *   走 Rust 侧 Resized 挂点，拖拽期间经"用户调整中"标志暂停自适应
 *   set_size（防高度被程序拉回，见 hud-resize.ts / session_hud.rs）；
 *   内容超高时列表区纵向滚动；
 * - 语言/主题同步：文案为轻量双语词典（zh/en），语言偏好读主面板写入
 *   的 localStorage 键（同源 WebView 共享；缺失/异常回退 zh），不引入
 *   i18n 运行时；主题经本窗口 <html> 的 .dark 类切换 session-hud.html
 *   的 --hud-* 颜色变量 token（仅 "dark" 视为暗色，对齐 appearance.ts
 *   loadTheme 语义）。两者启动各读一次，随后监听主面板广播的
 *   zbar://appearance-changed（setLocale/applyTheme 发出，无 payload）
 *   即时跟随：语言变化全量重渲染，主题变化纯 CSS 生效无需重渲染。
 *
 * 尺寸栅格：Rust 侧按会话条数自适应窗口高度（hud_height），页面 CSS
 * 的头部/行/折叠行/留白尺寸必须与之一一对应（见 session-hud.html 注释）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { APPEARANCE_CHANGED_EVENT, THEME_KEY } from "./appearance";
import { LOCALE_KEY } from "./i18n/locale";
import { installHudResizeHandles } from "./hud-resize";

/** Rust 侧 HudSessionBrief（camelCase 契约，紧凑标量，无原始行数据）。
 *  行主体是"会话树"（主会话 + 全部子代理，注入版 V9 合计口径）：
 *  ↑/↓/⟲/Σ/× 均为树内 model_usage 合计，子代理不单独成行 */
interface HudSessionBrief {
  sessionId: string;
  /** 会话短标识（去 sess_ 前缀后 6 位） */
  short: string;
  /** 项目名（目录末段；缺失时为 "#短标识"） */
  project: string;
  /** 模型集合（逗号拼接，最近使用在前；null = 窗口内无请求） */
  models: string | null;
  /** 窗口内有活动的子代理数（>0 显示 ⟡n 徽标） */
  subCount: number;
  /** true = 生成中（树内任一成员最新一轮未完成） */
  generating: boolean;
  lastActiveAt: number;
  inTokens: number;
  outTokens: number;
  cacheRead: number;
  /** 全生命周期累计 Σ = ↑+↓+⟲ */
  total: number;
  /** 模型请求笔数（model_usage 行数） */
  reqCount: number;
  /** 最新完成模型请求的速度；null → "–"。 */
  speed: number | null;
  /** generation = 可信首字到完成；request_average = 仅请求总耗时的近似。 */
  speedQuality?: "generation" | "request_average" | null;
  /** 速度所属请求的完成时刻。 */
  speedCompletedAt?: number | null;
  /** measuring = 生成中但本轮尚无已确认速度。 */
  speedState?: "measuring" | "recent" | "unavailable";
  /** 最近一笔完成请求 TTFT（毫秒，静态参考）；null → "–" */
  ttftMs: number | null;
}

/** Rust 侧 HudModelSpeed（模型速度区单行，按模型聚合的窗口速度摘要） */
interface HudModelSpeed {
  /** 模型 id（原值，行内 truncate + title 全名） */
  model: string;
  /** 最近一笔完成请求的输出速度 t/s */
  tps: number;
  /** 窗口内全部可信样本的总输出速度 t/s */
  avgTps: number;
  /** 窗口内可信样本的单笔最快速度 t/s（与 avgTps 同样本池） */
  maxTps: number;
  /** 窗口内可信样本的单笔最慢速度 t/s（与 avgTps 同样本池） */
  minTps: number;
  /** 可信速度样本数 */
  samples: number;
}

/** Rust 侧 HudSnapshot */
interface HudSnapshot {
  v: number;
  /** 窗口内活跃会话总数（含未展示的，折叠提示消费） */
  totalActive: number;
  sessions: HudSessionBrief[];
  /** 今日合计（自然日本地零点起全库 model_usage；窗口级汇总，随
   *  showTokens 配置隐藏——不想看数字的用户整行不显示；全 0 = 今日
   *  无请求不显示该行） */
  todayTotal: {
    in: number;
    out: number;
    cacheRead: number;
    total: number;
    reqCount: number;
  };
  /** 模型速度区（活跃窗口内按模型分组的速度摘要，最近使用降序至多
   *  3 个；空数组 = 不渲染该区，显隐与窗口高度判定同条件，见
   *  session_hud.rs model_speed_visible） */
  modelSpeeds: HudModelSpeed[];
}

/** Rust 侧热推参数（设置卡变更 / 悬浮窗设置面板两个滑块落盘 →
 *  zbar://session-hud-params） */
interface HudParams {
  opacity: number;
  windowMinutes: number;
  showTokens: boolean;
  showModel: boolean;
  /** 字体缩放（0.8~1.4，header 滑块 ↔ fontScale 配置双向同步） */
  fontScale: number;
}

/** 初始配置（get_session_hud_config 返回，含热推参数之外的字段） */
interface HudConfigFull extends HudParams {
  enabled: boolean;
  pos: [number, number] | null;
  /** 窗口宽度（逻辑 px；用户拖拽持久化或默认值） */
  width: number;
  /** 窗口高度（逻辑 px）：null = 从未拖拽（自适应高度模式），
   *  有值 = 用户拖拽过（自由尺寸模式，列表超高滚动） */
  height: number | null;
}

/** 词典行结构（以 zh 为基准，en 必须同构） */
interface Msg {
  title: string;
  active: (n: number) => string;
  empty: string;
  more: (n: number) => string;
  generating: string;
  idle: string;
  /** 子代理徽标 tooltip */
  subBadge: (n: number) => string;
  /** 速度位"测算中"占位 tooltip（生成中但尚无可测数据） */
  measuring: string;
  /** 行2 核心指标行（对齐注入版 renderSessionBar 字段序 + 补 TTFT） */
  metricsLine: (
    total: string,
    req: string,
    speed: string,
    ttft: string
  ) => string;
  /** 行3 分解行 */
  splitLine: (inT: string, out: string, cr: string) => string;
  /** 设置面板字体行 tooltip / 无障碍名（"拖动调整文字大小"） */
  fontTip: string;
  /** 设置面板字体行标签（"字体"） */
  fontLabel: string;
  /** 设置面板透明度行标签（"透明度"） */
  opacityLabel: string;
  /** 透明度滑块 tooltip / 无障碍名（"拖动调整窗口不透明度"） */
  opacityTip: string;
  /** 设置按钮 tooltip / 无障碍名（"设置"） */
  settingsTip: string;
  /** 关闭按钮 tooltip / 无障碍名（"关闭悬浮窗"） */
  closeTip: string;
  /** 模型速度区行内跟随小字（最近值之后的 均 / 快 / 慢） */
  modelSpeedExtra: (avg: string, max: string, min: string) => string;
  /** 模型速度区行 title 详情（模型全名 + 最近值/窗口均/最快/最慢/样本数 +
   *  行内颜色分档口径说明） */
  modelSpeedTitle: (
    model: string,
    recent: string,
    avg: string,
    max: string,
    min: string,
    n: number
  ) => string;
  /** 今日合计行（窗口级汇总：今日 Σ 总量 · × 请求数） */
  todayLine: (total: string, req: string) => string;
  /** 今日合计行 title 详情（in 为 input 原值含缓存读，中性符号避免与
   *  行内 ↑ 非缓存输入的语义混淆） */
  todayDetail: (inT: string, out: string, cr: string, req: string) => string;
}

/** 轻量双语词典（仅悬浮窗内文案；语言偏好与主面板同键共享） */
const MESSAGES: Record<"zh" | "en", Msg> = {
  zh: {
    title: "ZCode 会话",
    active: (n: number) => `${n} 活跃`,
    empty: "暂无活跃会话",
    more: (n: number) => `还有 ${n} 个会话`,
    generating: "生成中",
    idle: "空闲",
    subBadge: (n: number) => `${n} 个子代理在跑（已并入本会话合计）`,
    measuring: "生成中，请求完成后显示速率",
    metricsLine: (total, req, speed, ttft) =>
      `Σ ${total} · × ${req} · ${speed} t/s · TTFT ${ttft}`,
    splitLine: (inT, out, cr) => `↑ ${inT}  ↓ ${out}  ⟲ ${cr}`,
    fontTip: "拖动调整文字大小",
    fontLabel: "字体",
    opacityLabel: "透明度",
    opacityTip: "拖动调整窗口不透明度",
    settingsTip: "设置",
    closeTip: "关闭悬浮窗",
    modelSpeedExtra: (avg, max, min) => `· 均 ${avg} · 快 ${max} · 慢 ${min}`,
    modelSpeedTitle: (model, recent, avg, max, min, n) =>
      `${model} · 最近 ${recent} t/s · 均 ${avg} t/s · 快 ${max} t/s · 慢 ${min} t/s · ${n} 笔；颜色按最近值分档`,
    todayLine: (total, req) => `今日 Σ ${total} · × ${req}`,
    todayDetail: (inT, out, cr, req) =>
      `输入(含⟲) ${inT} · 输出 ${out} · 缓存读 ${cr} · 请求 ${req}`,
  },
  en: {
    title: "ZCode Sessions",
    active: (n: number) => `${n} active`,
    empty: "No active sessions",
    more: (n: number) => `${n} more`,
    generating: "Generating",
    idle: "Idle",
    subBadge: (n: number) => `${n} subagent(s) running (merged into totals)`,
    measuring: "Generating — rate appears once the request completes",
    metricsLine: (total, req, speed, ttft) =>
      `Σ ${total} · × ${req} · ${speed} t/s · TTFT ${ttft}`,
    splitLine: (inT, out, cr) => `↑ ${inT}  ↓ ${out}  ⟲ ${cr}`,
    fontTip: "Drag to adjust text size",
    fontLabel: "Font",
    opacityLabel: "Opacity",
    opacityTip: "Drag to adjust window opacity",
    settingsTip: "Settings",
    closeTip: "Close overlay",
    modelSpeedExtra: (avg, max, min) =>
      `· avg ${avg} · best ${max} · worst ${min}`,
    modelSpeedTitle: (model, recent, avg, max, min, n) =>
      `${model} · recent ${recent} t/s · avg ${avg} t/s · best ${max} t/s · worst ${min} t/s · ${n} requests; color tiers by the latest value`,
    todayLine: (total, req) => `Today Σ ${total} · × ${req}`,
    todayDetail: (inT, out, cr, req) =>
      `Input (incl. cache read) ${inT} · Output ${out} · Cache read ${cr} · Requests ${req}`,
  },
};

/** 读语言偏好（主面板 locale.ts 同键同回退：异常/非法值一律 zh）。
 *  返回词典对象引用（全局仅两份），调用方以引用对比判断语言是否变化 */
function detectMsg(): Msg {
  try {
    const v = localStorage.getItem(LOCALE_KEY);
    return v === "en" ? MESSAGES.en : MESSAGES.zh;
  } catch {
    return MESSAGES.zh;
  }
}

/** 应用主题（主面板 appearance.ts loadTheme 同语义）：仅 "dark" 视为
 *  暗色，其余（含无值/损坏值/读取异常）一律亮色。给本窗口 <html> 切
 *  .dark 类，session-hud.html 的 --hud-* 变量在两套 token 间整体切换，
 *  纯 CSS 生效，无需重渲染 */
function applyHudTheme(): void {
  let dark = false;
  try {
    dark = localStorage.getItem(THEME_KEY) === "dark";
  } catch {
    dark = false; /* localStorage 异常一律亮色（对齐 loadTheme） */
  }
  document.documentElement.classList.toggle("dark", dark);
}

/** token 数缩写（注入版 fmtTokens 同语义）：恒定宽度紧凑格式，
 *  998 / 1.2k / 10.5M（千位以下原样） */
function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(Math.round(n));
}

/** 速度文案：只接受正的已确认数值；请求平均值明确用 ≈ 标记。 */
function fmtSpeed(
  speed: number | null,
  quality?: "generation" | "request_average" | null
): string {
  if (speed == null || !Number.isFinite(speed) || speed <= 0) return "–";
  const prefix = quality === "request_average" ? "≈" : "";
  return `${prefix}${speed.toFixed(1)}`.padStart(4, " ");
}

/** TTFT 文案：null（缺失/进行中）→ "–"，否则秒一位小数（如 5.8s）
 *  并按 4 字符补位 */
function fmtTtft(ttftMs: number | null): string {
  const s = ttftMs == null ? "–" : `${(ttftMs / 1000).toFixed(1)}s`;
  return s.padStart(4, " ");
}

// ===== 运行时状态 =====

/** 当前词典（模块级可变状态）：启动读一次，此后随 zbar://appearance-changed
 *  重读 localStorage 更新（渲染取词统一走本变量） */
let msg: Msg = detectMsg();
let cfg: HudParams = {
  opacity: 0.92,
  windowMinutes: 10,
  showTokens: true,
  showModel: true,
  fontScale: 1.0,
};
let snapshot: HudSnapshot | null = null;

const rootEl = document.getElementById("hud-root");
const listEl = document.getElementById("hud-list");
const emptyEl = document.getElementById("hud-empty");
const moreEl = document.getElementById("hud-more");
const todayEl = document.getElementById("hud-today");
const modelsEl = document.getElementById("hud-models");
const countEl = document.getElementById("hud-count");
const titleEl = document.getElementById("hud-title");
const fontSliderEl = document.getElementById(
  "hud-font-slider"
) as HTMLInputElement | null;
const fontBoxEl = document.getElementById("hud-font-box");
const fontLabelEl = document.getElementById("hud-font-label");
const fontValueEl = document.getElementById("hud-font-value");
const opacitySliderEl = document.getElementById(
  "hud-opacity-slider"
) as HTMLInputElement | null;
const opacityBoxEl = document.getElementById("hud-opacity-box");
const opacityLabelEl = document.getElementById("hud-opacity-label");
const opacityValueEl = document.getElementById("hud-opacity-value");
const settingsEl = document.getElementById("hud-settings");
const settingsBtnEl = document.getElementById("hud-settings-btn");
const closeBtnEl = document.getElementById("hud-close-btn");

/** 字体缩放落盘防抖 timer（滑块拖动连续触发，300ms 合并提交一次） */
let fontSaveTimer: number | undefined = undefined;
/** 热推回填滑块的防重入标志：用户拖动中收到回推（值相同）不重设，
 *  避免打断拖动手感 */
let fontSliderDirty = false;
/** 透明度落盘防抖 timer 与拖动中的回填防重入标志（与字体滑块同款） */
let opacitySaveTimer: number | undefined = undefined;
let opacitySliderDirty = false;

/** 应用字体缩放：写根节点 CSS 变量 --hud-scale，页面全部字号/行高/
 *  行高栅格 calc 随动（Rust 侧自适应高度同乘 font_scale，见
 *  session_hud.rs hud_height）。脏值按 1.0 处理（与 Rust clamp 对齐）；
 *  读数回填（滑块刻度在拖动中不覆盖，保持手感） */
function applyFontScale(scale: number): void {
  if (!rootEl) return;
  const v =
    Number.isFinite(scale) && scale >= 0.8 && scale <= 1.4 ? scale : 1.0;
  rootEl.style.setProperty("--hud-scale", String(v));
  if (fontSliderEl && document.activeElement !== fontSliderEl) {
    fontSliderEl.value = String(v);
  }
  if (fontValueEl) fontValueEl.textContent = `${v.toFixed(2)}×`;
}

/** 滑块输入 → 即时缩放 + 300ms 防抖落盘（专用轻量命令
 *  set_session_hud_font_scale：只改 fontScale 并热推，不走建/关窗流程） */
function handleFontSliderInput(): void {
  if (!fontSliderEl) return;
  const v = parseFloat(fontSliderEl.value);
  if (!Number.isFinite(v)) return;
  fontSliderDirty = true;
  applyFontScale(v);
  if (fontSaveTimer !== undefined) window.clearTimeout(fontSaveTimer);
  fontSaveTimer = window.setTimeout(() => {
    fontSaveTimer = undefined;
    fontSliderDirty = false;
    invoke("set_session_hud_font_scale", { scale: v }).catch(() => {
      /* 落盘失败静默：本次缩放已生效，下次拖动再试 */
    });
  }, 300);
}

/** 透明度读数与滑块刻度回填（刻度 = 配置值百分比；滑块聚焦/拖动中不
 *  覆盖刻度，避免打断手感——与字体滑块同款） */
function syncOpacityReadout(): void {
  const pct = Math.round(cfg.opacity * 100);
  if (opacityValueEl) opacityValueEl.textContent = `${pct}%`;
  if (opacitySliderEl && document.activeElement !== opacitySliderEl) {
    opacitySliderEl.value = String(pct);
  }
}

/** 透明度滑块输入 → 即时应用 + 300ms 防抖落盘（专用轻量命令
 *  set_session_hud_opacity：只改 opacity 并热推，不走建/关窗流程；热推
 *  回环带回的值与本地一致，读数与刻度不会抖动） */
function handleOpacitySliderInput(): void {
  if (!opacitySliderEl) return;
  const pct = parseFloat(opacitySliderEl.value);
  if (!Number.isFinite(pct)) return;
  opacitySliderDirty = true;
  cfg = { ...cfg, opacity: pct / 100 };
  applyOpacity();
  if (opacityValueEl) opacityValueEl.textContent = `${Math.round(pct)}%`;
  if (opacitySaveTimer !== undefined) window.clearTimeout(opacitySaveTimer);
  opacitySaveTimer = window.setTimeout(() => {
    opacitySaveTimer = undefined;
    opacitySliderDirty = false;
    invoke("set_session_hud_opacity", { opacity: pct / 100 }).catch(() => {
      /* 落盘失败静默：本次调整已生效，下次拖动再试 */
    });
  }, 300);
}

function applyOpacity(): void {
  if (!rootEl) return;
  // 面板打开期间按"应用值"钳制不低于 PANEL_SETTINGS_MIN_OPACITY（设置页
  // 的透明度入口已移除，窗口被拖到 25% 后若面板自身也随透明变淡，读数与
  // 滑块将难以辨认，等于失去找回通道）；收起后立即恢复用户设置值。落盘
  // 值始终是用户拖动的真实刻度（见 handleOpacitySliderInput），钳制只
  // 作用在根节点 CSS opacity 上
  const applied = settingsOpen()
    ? Math.max(cfg.opacity, PANEL_SETTINGS_MIN_OPACITY)
    : cfg.opacity;
  rootEl.style.opacity = String(applied);
}

/** 面板打开期间的最低应用不透明度（P1-2）：仅是"应用值"下限，不是落盘
 *  值、也非合法域（Rust 侧合法域仍是 0.25~1.0）。0.5 下窗口内容清晰可读，
 *  又能让用户看出自己设的更淡效果（下次收起面板后生效） */
const PANEL_SETTINGS_MIN_OPACITY = 0.5;

/** 设置面板是否展开（DOM 类为准，避免与 CSS 判定漂移） */
function settingsOpen(): boolean {
  return settingsEl?.classList.contains("open") ?? false;
}

/** 设置面板显隐（按钮点击开关；外部点击 / Esc 收起由 main() 的 document
 *  监听处理）。开合只切换 class（display:none ↔ flex），不改变窗口尺寸；
 *  随即重算应用不透明度（打开期间有下限钳制，见 applyOpacity） */
function setSettingsOpen(open: boolean): void {
  settingsEl?.classList.toggle("open", open);
  settingsBtnEl?.classList.toggle("open", open);
  // aria-expanded 跟随（无障碍：按钮声明自己控制的面板是否展开）
  settingsBtnEl?.setAttribute("aria-expanded", open ? "true" : "false");
  applyOpacity();
}

/** 头部按钮公共接线：阻断 mousedown/pointerdown 冒泡（不触发窗口拖动，
 *  与既有滑块同款双保险）后执行 handler */
function wireHeaderButton(el: HTMLElement | null, handler: () => void): void {
  if (!el) return;
  const stop = (e: Event) => e.stopPropagation();
  el.addEventListener("mousedown", stop);
  el.addEventListener("pointerdown", stop);
  el.addEventListener("click", handler);
}

/** 面板滑块公共接线：同上阻断冒泡 + input 处理器 */
function wireSlider(el: HTMLInputElement | null, handler: () => void): void {
  if (!el) return;
  const stop = (e: Event) => e.stopPropagation();
  el.addEventListener("mousedown", stop);
  el.addEventListener("pointerdown", stop);
  el.addEventListener("input", handler);
}

/** 静态文案（header 标题 / 设置面板标签与 tooltip / 两个滑块与按钮的
 *  无障碍名 / 首帧空态）按当前语言应用：启动与语言切换共用。动态区
 *  （计数/列表/今日行/模型速度区/折叠行/两个滑块读数）的文案在
 *  renderShell/renderRows/applyFontScale 等处每次写入，不经此 */
function applyStaticText(): void {
  if (titleEl) titleEl.textContent = msg.title;
  if (fontBoxEl) fontBoxEl.title = msg.fontTip; // 字体行 tooltip 随语言
  fontSliderEl?.setAttribute("aria-label", msg.fontTip); // 无可见标签，补无障碍名
  if (fontLabelEl) fontLabelEl.textContent = msg.fontLabel;
  if (opacityBoxEl) opacityBoxEl.title = msg.opacityTip;
  opacitySliderEl?.setAttribute("aria-label", msg.opacityTip);
  if (opacityLabelEl) opacityLabelEl.textContent = msg.opacityLabel;
  if (settingsEl) settingsEl.setAttribute("aria-label", msg.settingsTip);
  if (settingsBtnEl) {
    // 设置按钮：title（hover 提示）与 aria-label 同文案
    settingsBtnEl.title = msg.settingsTip;
    settingsBtnEl.setAttribute("aria-label", msg.settingsTip);
  }
  if (closeBtnEl) {
    closeBtnEl.title = msg.closeTip;
    closeBtnEl.setAttribute("aria-label", msg.closeTip);
  }
  if (emptyEl) emptyEl.textContent = msg.empty; // 数据到达前的首帧空态
}

/** 空态/列表/模型速度区/今日行/折叠行五者的显隐与文案。今日合计行
 *  为窗口级汇总（自然日本地零点起全库合计），模型速度区为按模型分组
 *  的窗口速度摘要——两者都属数字信息，随 showTokens 配置隐藏（Rust 侧
 *  窗口高度判定同条件，见 session_hud.rs today_visible /
 *  model_speed_visible）；仅在开启数据行、有可见会话且各有数据时显示
 *  （空态不显示，跟随列表存在）。圆角闭环：折叠行显示时折叠行承担
 *  底部圆角；否则最下方可见行承担，优先级 今日行 > 模型速度区 > 列表 */
function renderShell(): void {
  if (!listEl || !emptyEl || !moreEl || !countEl || !todayEl || !modelsEl) return;
  const sessions = snapshot?.sessions ?? [];
  const totalActive = snapshot?.totalActive ?? 0;
  const today = snapshot?.todayTotal;
  const modelSpeeds = snapshot?.modelSpeeds ?? [];
  const showToday =
    cfg.showTokens &&
    sessions.length > 0 &&
    today != null &&
    today.total > 0;
  // 与 Rust 侧 model_speed_visible 逐字同条件
  const showModels =
    cfg.showTokens && modelSpeeds.length > 0 && sessions.length > 0;
  // 头部计数与空态严格互斥：以可见列表为准（Rust 侧保证 total_active>0
  // ⟹ sessions 非空，此处按列表存在性双重防御，计数绝不与"暂无活跃
  // 会话"同屏）
  countEl.textContent =
    sessions.length > 0 && totalActive > 0 ? msg.active(totalActive) : "";
  if (sessions.length === 0) {
    listEl.replaceChildren();
    listEl.classList.remove("rounded-bottom");
    emptyEl.textContent = msg.empty;
    emptyEl.classList.remove("hidden");
    moreEl.style.display = "none";
    todayEl.style.display = "none";
    renderModels(false);
    return;
  }
  emptyEl.classList.add("hidden");
  const overflow = totalActive - sessions.length;
  if (overflow > 0) {
    moreEl.textContent = msg.more(overflow);
    moreEl.style.display = "flex";
    listEl.classList.remove("rounded-bottom");
    todayEl.classList.remove("rounded-bottom");
    modelsEl.classList.remove("rounded-bottom");
  } else {
    moreEl.style.display = "none";
    // 底部圆角由最下方可见行承担：今日行 > 模型速度区 > 列表
    todayEl.classList.toggle("rounded-bottom", showToday);
    modelsEl.classList.toggle("rounded-bottom", !showToday && showModels);
    listEl.classList.toggle("rounded-bottom", !showToday && !showModels);
  }
  renderModels(showModels);
  if (showToday && today) {
    todayEl.textContent = msg.todayLine(
      fmtTokens(today.total),
      String(today.reqCount)
    );
    todayEl.title = msg.todayDetail(
      String(today.in),
      String(today.out),
      String(today.cacheRead),
      String(today.reqCount)
    );
    todayEl.style.display = "flex";
  } else {
    todayEl.style.display = "none";
  }
}

/**
 * 字段级变化高亮（注入版式"数值跳动"感知补足）：每帧保存各会话的
 * 展示字段值（Σ/↑/↓/⟲/×/速度/TTFT 的最终显示串），渲染时 diff，
 * 变化字段的 span 加 .hud-flash（新元素插入即播一次 0.55s 渐隐动画，
 * 1 秒节拍下同字段连续变化可重复触发）。首帧无上一帧参照，不闪。
 */
type FieldVals = [
  string, // Σ total
  string, // ↑ in
  string, // ↓ out
  string, // ⟲ cacheRead
  string, // × req
  string, // speed
  string, // ttft
];
const prevFields = new Map<string, FieldVals>();

/** 字段 span：changed 时加 .hud-flash */
function fieldSpan(text: string, changed: boolean, cls?: string): HTMLSpanElement {
  const el = document.createElement("span");
  if (cls) el.className = cls;
  el.textContent = text;
  if (changed) el.classList.add("hud-flash");
  return el;
}

/** 模型速度三档变色（与主面板速度卡同阈值）：≥70 绿 / 40–70 黄 /
 *  <40 红 */
function modelSpeedClass(tps: number): string {
  if (tps >= 70) return "hud-ms-fast";
  if (tps >= 40) return "hud-ms-mid";
  return "hud-ms-slow";
}

/** 重建模型速度区（列表下方、今日行之上，每活跃模型一行：左模型名
 *  truncate、右速度值右对齐三档变色 + 小号灰单位后缀 + 更暗一档的
 *  "· 均 x · 快 y · 慢 z" 跟随小字）。textContent 写入外部字符串，无
 *  innerHTML 注入面；速度属数字信息，显隐随 showTokens（与 Rust 侧
 *  model_speed_visible 同条件：开启数据行 + 有样本 + 有可见会话） */
function renderModels(visible: boolean): void {
  if (!modelsEl) return;
  if (!visible) {
    modelsEl.replaceChildren();
    modelsEl.style.display = "none";
    return;
  }
  const rows = (snapshot?.modelSpeeds ?? []).map((m) => {
    const row = document.createElement("div");
    row.className = "hud-msrow";
    const recent = m.tps.toFixed(1);
    const avg = m.avgTps.toFixed(1);
    const max = m.maxTps.toFixed(1);
    const min = m.minTps.toFixed(1);
    // 行 title 同时兜底模型全名（名字截断时）与速度口径
    row.title = msg.modelSpeedTitle(m.model, recent, avg, max, min, m.samples);
    const name = document.createElement("span");
    name.className = "hud-ms-name";
    name.textContent = m.model;
    const val = document.createElement("span");
    val.className = `hud-ms-val ${modelSpeedClass(m.tps)}`;
    val.textContent = recent;
    // 单位后缀：独立静态节点，只继承行灰、小一号，不参与三档变色
    //（同会话行"数值 + t/s"格式；flex 行内用 span 承载以便单独设字号；
    // 字号随 --hud-scale 缩放与行内其余文字保持比例）
    const unit = document.createElement("span");
    unit.textContent = " t/s";
    unit.style.fontSize = "calc(8px * var(--hud-scale))";
    unit.style.whiteSpace = "pre";
    // 均 / 快 / 慢跟随小字（与最近值同样本池）：取整求紧凑（行内示例
    // "均 65 · 快 120 · 慢 40"），一位小数全量口径在行 title 里；窄窗口
    // 放不下时由 CSS 裁切兜底
    const extra = document.createElement("span");
    extra.className = "hud-ms-extra";
    extra.textContent = msg.modelSpeedExtra(
      m.avgTps.toFixed(0),
      m.maxTps.toFixed(0),
      m.minTps.toFixed(0)
    );
    row.append(name, val, unit, extra);
    return row;
  });
  modelsEl.replaceChildren(...rows);
  modelsEl.style.display = rows.length > 0 ? "flex" : "none";
}

/** 重建会话行（textContent 写入外部字符串，无 innerHTML 注入面） */
function renderRows(): void {
  if (!listEl) return;
  const sessions = snapshot?.sessions ?? [];
  const rows = sessions.map((s) => {
    const row = document.createElement("div");
    row.className = "hud-row";
    row.title = `${s.project} · ${s.sessionId}`;

    // 本帧展示字段值（存显示串：只有用户可见的变化才触发高亮）
    const cur: FieldVals = [
      fmtTokens(s.total),
      fmtTokens(s.inTokens),
      fmtTokens(s.outTokens),
      fmtTokens(s.cacheRead),
      String(s.reqCount).padStart(3, "0"),
      fmtSpeed(s.speed, s.speedQuality),
      fmtTtft(s.ttftMs),
    ];
    const prev = prevFields.get(s.sessionId);
    const changed = (i: number): boolean => prev !== undefined && prev[i] !== cur[i];

    // 首行：状态点 + 项目名 + 短标识 +（可选）子代理徽标 +（可选）模型
    const top = document.createElement("div");
    top.className = "hud-row-top";
    const dot = document.createElement("span");
    dot.className = `hud-dot${s.generating ? " gen" : ""}`;
    dot.title = s.generating ? msg.generating : msg.idle;
    const project = document.createElement("span");
    project.className = "hud-project";
    project.textContent = s.project;
    const short = document.createElement("span");
    short.className = "hud-short";
    short.textContent = s.short;
    top.append(dot, project, short);
    // 子代理徽标：窗口内有活动的子代理数 >0 才占位（克制：小号淡紫）
    if (s.subCount > 0) {
      const sub = document.createElement("span");
      sub.className = "hud-sub";
      sub.textContent = `⟡${s.subCount}`;
      sub.title = msg.subBadge(s.subCount);
      top.append(sub);
    }
    // 模型集合：直接渲染全量逗号拼接串（注入版 models 口径，Rust 侧
    // 已按最近使用降序），多模型时由 CSS 省略号自然裁切，title 给全量
    if (cfg.showModel && s.models) {
      const model = document.createElement("span");
      model.className = "hud-model";
      model.textContent = s.models;
      model.title = s.models;
      top.append(model);
    }

    // 行2 核心指标行 + 行3 分解行（可关，随 showTokens 整组隐藏）：
    // 对齐注入版会话条字段序并补 TTFT。等宽补位（fmtTokens 恒定宽度 /
    // req padStart(3) / speed、ttft padStart(4)）配合 CSS tabular-nums
    // 保持行宽稳定；行内 nowrap + overflow hidden 由 CSS 防御极端长值。
    // 各字段值为独立 span（分隔符是静态文本节点），变化字段挂 .hud-flash
    if (cfg.showTokens) {
      const metrics = document.createElement("div");
      metrics.className = "hud-row-data";
      // Σ 稍放大做视觉锚点（.hud-sigma），其余字段 9px。
      // 速度位"测算中"占位：生成中但无窗口数据且无保持值（speed 为
      // null）→ 呼吸动画 + tooltip（回答完成 1 秒内显示真实速率并触发
      // 既有 flash 高亮——"–"到数值的跳变走字段级 diff）
      const measuring =
        s.speedState === "measuring" || (s.speed == null && s.generating);
      const speedEl = fieldSpan(cur[5], changed(5));
      if (measuring) {
        speedEl.classList.add("hud-measuring");
        speedEl.title = msg.measuring;
      }
      metrics.append(
        fieldSpan(`Σ ${cur[0]}`, changed(0), "hud-sigma"),
        document.createTextNode(" · "),
        fieldSpan(`× ${cur[4]}`, changed(4)),
        document.createTextNode(" · "),
        speedEl,
        // 单位放静态文本节点（flash 只作用于数字）；"测算中"占位态
        // 同样带单位（"– t/s"），与数值态行宽一致
        document.createTextNode(" t/s"),
        document.createTextNode(" · TTFT "),
        fieldSpan(cur[6], changed(6))
      );
      metrics.title = msg.metricsLine(
        String(s.total),
        String(s.reqCount),
        fmtSpeed(s.speed, s.speedQuality).trim(),
        fmtTtft(s.ttftMs).trim()
      );

      const split = document.createElement("div");
      split.className = "hud-row-data";
      split.append(
        document.createTextNode("↑ "),
        fieldSpan(cur[1], changed(1)),
        document.createTextNode("  ↓ "),
        fieldSpan(cur[2], changed(2)),
        document.createTextNode("  ⟲ "),
        fieldSpan(cur[3], changed(3))
      );
      split.title = msg.splitLine(
        String(s.inTokens),
        String(s.outTokens),
        String(s.cacheRead)
      );

      row.append(top, metrics, split);
    } else {
      row.append(top);
    }
    prevFields.set(s.sessionId, cur);
    return row;
  });
  listEl.replaceChildren(...rows);
  // 清理已消失会话的 diff 状态（防 Map 随历史会话无限增长）
  if (prevFields.size > sessions.length) {
    const ids = new Set(sessions.map((s) => s.sessionId));
    for (const id of prevFields.keys()) {
      if (!ids.has(id)) prevFields.delete(id);
    }
  }
}

function render(): void {
  applyOpacity();
  renderShell();
  renderRows();
}

const main = async () => {
  if (!rootEl || !listEl || !emptyEl || !moreEl || !todayEl || !modelsEl) return;
  // 冷启动同步主题（语言已在模块顶层 detectMsg 读取）：读主面板写入的
  // zbar-theme 切 .dark，主面板主题切换时经下方 appearance-changed 重读
  applyHudTheme();
  applyStaticText();

  // 拖拽热区层（8 向隐形热区）：undecorated 窗口的系统边缘热区在 Windows
  // 上不生效，尺寸调整全靠自绘热区（见 hud-resize.ts）。纯 DOM 安装，
  // 无窗口 API 调用，失败不影响其余功能
  installHudResizeHandles();

  // 设置面板两个滑块：mousedown/pointerdown 阻断冒泡，防止拖动滑块触发
  // 窗口拖动（data-tauri-drag-region 依赖 mousedown；input 元素本身在
  // Tauri 拖拽区判定中已不触发拖动，此处双保险防宿主实现差异）；
  // input 即时生效 + 防抖落盘（见 handleFontSliderInput /
  // handleOpacitySliderInput）
  wireSlider(fontSliderEl, handleFontSliderInput);
  wireSlider(opacitySliderEl, handleOpacitySliderInput);

  // 头部按钮：设置按钮开关面板；关闭按钮经 close_session_hud 关闭悬浮窗
  // 功能（后端总开关置 false = 停轮询 + 关窗，与设置页总开关关闭同一条
  // 路径）。窗口随后销毁，invoke 响应不再需要（失败静默：窗口仍在时用户
  // 可再点一次）
  wireHeaderButton(settingsBtnEl, () => {
    setSettingsOpen(!settingsOpen());
  });
  wireHeaderButton(closeBtnEl, () => {
    invoke("close_session_hud").catch(() => {
      /* 窗口即将销毁：响应丢失属预期；异常时保持窗口不动 */
    });
  });

  // 面板外点击收起：document 捕获阶段监听（早于热区 pointerdown 等一切
  // 目标处理器），点击落在面板内或设置按钮上时跳过（按钮自身的开关逻辑
  // 在冒泡阶段执行，两段互不冲突）。常驻监听，随窗口销毁回收
  document.addEventListener(
    "pointerdown",
    (e) => {
      const target = e.target as Node | null;
      if (!target) return;
      if (settingsEl?.contains(target) || settingsBtnEl?.contains(target)) {
        return;
      }
      setSettingsOpen(false);
    },
    true
  );

  // Esc 收起面板（P2-2）：悬浮窗是 focusable(false) 的不抢焦点窗口，键盘
  // 焦点在多数场景不落在本页（Esc 未必送达），属低成本兜底——焦点在
  // WebView 内时（预览 / 特殊宿主）可用；面板未开时调用幂等无副作用
  document.addEventListener("keydown", (e) => {
    if (e.key === "Escape") setSettingsOpen(false);
  });

  // 初始配置（失败按默认渲染，防御式与 pet-main.ts 同风格）：fontScale
  // 写入 CSS 变量并回填滑块刻度、透明度回填读数与刻度
  try {
    const full = await invoke<HudConfigFull>("get_session_hud_config");
    cfg = {
      opacity: full.opacity,
      windowMinutes: full.windowMinutes,
      showTokens: full.showTokens,
      showModel: full.showModel,
      fontScale: full.fontScale,
    };
  } catch {
    /* 静默：保持默认参数 */
  }
  applyFontScale(cfg.fontScale);
  syncOpacityReadout();
  render();

  // 数据流：轮询器推送的会话快照（内容变化才推，事件停流保持最后内容）
  try {
    await listen<HudSnapshot>("zbar://session-hud-usage", (e) => {
      snapshot = e.payload;
      render();
    });
  } catch {
    /* 监听失败保持空态（与 pet 同款降级） */
  }

  // 参数流：设置卡变更 / 滑块落盘回推（透明度/显示项/字体缩放即时生效）。
  // 拖动中（dirty）保留本地值不回填刻度——回推带回的是刚刚落盘的同一
  // 值，正常不会有视觉变化，保留本地值只为防在途旧回推打断手感
  try {
    await listen<HudParams>("zbar://session-hud-params", (e) => {
      const next: HudParams = { ...cfg, ...e.payload };
      if (opacitySliderDirty) next.opacity = cfg.opacity;
      cfg = next;
      if (!fontSliderDirty || document.activeElement !== fontSliderEl) {
        applyFontScale(cfg.fontScale);
      }
      if (!opacitySliderDirty || document.activeElement !== opacitySliderEl) {
        syncOpacityReadout();
      }
      render(); // render → applyOpacity 应用 cfg.opacity
    });
  } catch {
    /* 静默：参数保持初始值 */
  }

  // 外观同步流：主面板语言（setLocale）/主题（applyTheme）切换后广播
  // zbar://appearance-changed（无 payload，接收方重读 localStorage 自行
  // 对比；常量与主面板共用 appearance.ts 单一来源）。语言变化：更新模块
  // 级词典引用 + 静态文案 + 全量重渲染；主题变化：纯 CSS 变量切换，
  // applyHudTheme 切 .dark 即生效，无需重渲染。主面板启动首帧的
  // applyTheme 也会广播一次（HUD 未创建则事件无人接收），重读对比后
  // 幂等无副作用。与既有 listen 同款 try-catch 降级：监听失败保持启动
  // 时读取的偏好（HUD 为长生命周期窗口，随窗口销毁自动清理，无需手动
  // unlisten）
  try {
    await listen(APPEARANCE_CHANGED_EVENT, () => {
      applyHudTheme();
      const next = detectMsg();
      if (next !== msg) {
        msg = next;
        applyStaticText();
        render();
      }
    });
  } catch {
    /* 静默：保持启动时读取的语言与主题 */
  }
};

void main();

/**
 * 会话 Token 悬浮窗宿主壳（Session HUD）。
 *
 * 与宠物窗宿主壳（pet-main.ts）同款的零 React 薄壳：不引入 panel 的
 * 组件体系（widgets/layout 为 panel 专用），列表渲染在壳内直接完成。
 * 职责：
 * - 初始配置：invoke get_session_hud_config 读取（透明度 + 显示项 +
 *   活跃档位）；读取失败按 HTML/CSS 默认渲染（防御式，与 pet 同风格）；
 * - 数据流：监听 zbar://session-hud-usage（session_hud.rs 轮询器每
 *   2 秒/速度字段 1 秒节拍推送的会话快照，内容变化才推），重建列表 DOM；
 * - 参数流：监听 zbar://session-hud-params（设置卡变更热推），即时
 *   应用透明度与显示项，下一轮数据到达后列表按新显示项重建；
 * - 拖动：#hud-header 为 data-tauri-drag-region 拖动区（列表行不留
 *   拖动属性，避免后续行内 hover 交互误拖），位置由 Rust 侧 Moved
 *   挂点节流持久化；
 * - 文案：轻量双语词典（zh/en），语言偏好读主面板写入的 localStorage
 *   键（同源 WebView 共享；缺失/异常回退 zh），不引入 i18n 运行时。
 *
 * 尺寸栅格：Rust 侧按会话条数自适应窗口高度（hud_height），页面 CSS
 * 的头部/行/折叠行/留白尺寸必须与之一一对应（见 session-hud.html 注释）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

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
  /** 动态速度 t/s（最近一笔完成请求；生成中显示计算值、空闲归 0.0，
   *  null → "–"） */
  speed: number | null;
  /** 最近一笔完成请求 TTFT（毫秒，静态参考）；null → "–" */
  ttftMs: number | null;
}

/** Rust 侧 HudSnapshot */
interface HudSnapshot {
  v: number;
  /** 窗口内活跃会话总数（含未展示的，折叠提示消费） */
  totalActive: number;
  sessions: HudSessionBrief[];
}

/** Rust 侧热推参数（设置卡变更 → zbar://session-hud-params） */
interface HudParams {
  opacity: number;
  windowMinutes: number;
  showTokens: boolean;
  showModel: boolean;
}

/** 初始配置（get_session_hud_config 返回，含热推参数之外的字段） */
interface HudConfigFull extends HudParams {
  enabled: boolean;
  pos: [number, number] | null;
  width: number;
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
  },
};

/** 读语言偏好（主面板 locale.ts 同键；异常/非法值回退 zh） */
function detectMsg(): Msg {
  try {
    const v = localStorage.getItem("zbar-locale");
    return v === "en" ? MESSAGES.en : MESSAGES.zh;
  } catch {
    return MESSAGES.zh;
  }
}

/** token 数缩写（注入版 fmtTokens 同语义）：恒定宽度紧凑格式，
 *  998 / 1.2k / 10.5M（千位以下原样） */
function fmtTokens(n: number): string {
  if (!Number.isFinite(n) || n <= 0) return "0";
  if (n >= 1_000_000) return `${(n / 1_000_000).toFixed(1)}M`;
  if (n >= 1_000) return `${(n / 1_000).toFixed(1)}k`;
  return String(Math.round(n));
}

/** 速度文案：null（无最近完成轮/数据不足）→ "–"，否则一位小数并按
 *  4 字符补位（与 req padStart(3) 同思路，稳定行宽） */
function fmtSpeed(speed: number | null): string {
  return speed == null ? "–" : speed.toFixed(1).padStart(4, " ");
}

/** TTFT 文案：null（缺失/进行中）→ "–"，否则秒一位小数（如 5.8s）
 *  并按 4 字符补位 */
function fmtTtft(ttftMs: number | null): string {
  const s = ttftMs == null ? "–" : `${(ttftMs / 1000).toFixed(1)}s`;
  return s.padStart(4, " ");
}

// ===== 运行时状态 =====

const msg: Msg = detectMsg();
let cfg: HudParams = {
  opacity: 0.92,
  windowMinutes: 10,
  showTokens: true,
  showModel: true,
};
let snapshot: HudSnapshot | null = null;

const rootEl = document.getElementById("hud-root");
const listEl = document.getElementById("hud-list");
const emptyEl = document.getElementById("hud-empty");
const moreEl = document.getElementById("hud-more");
const countEl = document.getElementById("hud-count");
const titleEl = document.getElementById("hud-title");

function applyOpacity(): void {
  if (rootEl) {
    rootEl.style.opacity = String(cfg.opacity);
  }
}

/** 空态/列表/折叠行三者的显隐与文案 */
function renderShell(): void {
  if (!listEl || !emptyEl || !moreEl || !countEl) return;
  const sessions = snapshot?.sessions ?? [];
  const totalActive = snapshot?.totalActive ?? 0;
  countEl.textContent = totalActive > 0 ? msg.active(totalActive) : "";
  if (sessions.length === 0) {
    listEl.replaceChildren();
    listEl.classList.remove("rounded-bottom");
    emptyEl.textContent = msg.empty;
    emptyEl.classList.remove("hidden");
    moreEl.style.display = "none";
    return;
  }
  emptyEl.classList.add("hidden");
  const overflow = totalActive - sessions.length;
  if (overflow > 0) {
    moreEl.textContent = msg.more(overflow);
    moreEl.style.display = "flex";
    listEl.classList.remove("rounded-bottom");
  } else {
    moreEl.style.display = "none";
    listEl.classList.add("rounded-bottom");
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
      fmtSpeed(s.speed),
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
      const measuring = s.speed == null && s.generating;
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
        fmtSpeed(s.speed).trim(),
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
  if (!rootEl || !listEl || !emptyEl || !moreEl) return;
  if (titleEl) titleEl.textContent = msg.title;
  emptyEl.textContent = msg.empty; // 数据到达前的首帧空态

  // 初始配置（失败按默认渲染，防御式与 pet-main.ts 同风格）
  try {
    const full = await invoke<HudConfigFull>("get_session_hud_config");
    cfg = {
      opacity: full.opacity,
      windowMinutes: full.windowMinutes,
      showTokens: full.showTokens,
      showModel: full.showModel,
    };
  } catch {
    /* 静默：保持默认参数 */
  }
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

  // 参数流：设置卡变更热推（透明度/显示项即时生效）
  try {
    await listen<HudParams>("zbar://session-hud-params", (e) => {
      cfg = { ...cfg, ...e.payload };
      render();
    });
  } catch {
    /* 静默：参数保持初始值 */
  }
};

void main();

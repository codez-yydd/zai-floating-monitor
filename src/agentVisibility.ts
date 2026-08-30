/** 统计页的 Agent 展示偏好，仅影响界面展示，不影响本地采集和同步。 */
import type { MessageKey } from "./i18n";

export type AgentId =
  // 首批 5 个：本地会话/额度采集已接入
  | "zai"
  | "codex"
  | "claude"
  | "cursor"
  | "kimi"
  // 凭证驱动的新 provider（配额数据接入逐步上线；tab 需「已启用或有凭证」）
  | "gemini"
  | "grok"
  | "qoder"
  | "opencodego"
  | "minimax"
  | "moonshot"
  | "deepseek"
  | "longcat"
  | "mimo"
  | "alibaba"
  | "alibabatoken"
  | "stepfun"
  | "doubao";

/** 凭证驱动的新 provider 集合（tab 显示门槛 = 已启用或已有凭证）。 */
export type CredentialAgentId = Exclude<AgentId, "zai" | "codex" | "claude" | "cursor" | "kimi">;

export const CREDENTIAL_AGENTS: readonly CredentialAgentId[] = [
  "gemini",
  "grok",
  "qoder",
  "opencodego",
  "minimax",
  "moonshot",
  "deepseek",
  "longcat",
  "mimo",
  "alibaba",
  "alibabatoken",
  "stepfun",
  "doubao",
];

/**
 * presence 探测名单：CREDENTIAL_AGENTS 之外追加 claude/cursor——两者的
 * 「其他账号」额度区需要按「是否有凭证」决定补刷与 120s 轮询（探测结果
 * 进同一 credentialPresence state，DataCache 轮询自动覆盖）。
 * 注意：claude/cursor 属首批 5 个，tab 展示仍由纯偏好控制（StatsPanel 的
 * presence 门槛只对 isCredentialAgent 生效），本名单扩展不改变其 tab 行为；
 * 「有凭证自动开启」的边沿检测仍只遍历 CREDENTIAL_AGENTS。
 */
export const PRESENCE_PROVIDERS: readonly string[] = [
  ...CREDENTIAL_AGENTS,
  "claude",
  "cursor",
];

/** id 是否属于凭证驱动的新 provider（用于 tab 渲染分支与类型收窄）。 */
export function isCredentialAgent(id: string): id is CredentialAgentId {
  return (CREDENTIAL_AGENTS as readonly string[]).includes(id);
}

/**
 * 本地型 provider 集合：数据由后端直读本地文件/数据库（无凭证也能出数据），
 * tab 门槛 = 本地数据存在；凭证体系保持可用但为「可选」（不做强引导）。
 * gemini（Gemini CLI 的 OAuth 登录态）与 opencodego 同属本地直读型。
 */
export const LOCAL_AGENTS: readonly CredentialAgentId[] = ["opencodego", "gemini"];

/** id 是否属于本地型 provider（GenericQuotaPanel 据此切换凭证卡空态文案）。 */
export function isLocalAgent(id: string): boolean {
  return (LOCAL_AGENTS as readonly string[]).includes(id);
}

export type AgentVisibility = Record<AgentId, boolean>;

/** 各 Agent 的品牌展示色（汇总页与对比页共用，保证跨页视觉一致）。 */
export const AGENT_COLOR: Record<AgentId, string> = {
  zai: "#0ea5e9",
  codex: "#10a37f",
  claude: "#d97757",
  cursor: "#8b5cf6",
  kimi: "#4338ca",
  gemini: "#4285F4",
  grok: "#1d9bf0",
  qoder: "#6c5ce7",
  opencodego: "#f97316",
  minimax: "#00b96b",
  moonshot: "#4540d6",
  deepseek: "#4D6BFE",
  // 亮黄在暗色主题下文字对比度不足，正文色用深一档
  longcat: "#ffd100",
  mimo: "#ff6900",
  alibaba: "#ff6a00",
  alibabatoken: "#ff9240",
  stepfun: "#3b82f6",
  doubao: "#00d4aa",
};

/** 同一 Agent 的多账号系列用同色系色阶区分（明度递增，4 档，超出取最末档）。
 *  不用透明度方案：细柱上不可辨，且低透明度有"弱化"的语义误导。 */
export const AGENT_COLOR_SCALE: Record<AgentId, readonly string[]> = {
  zai: ["#0ea5e9", "#38bdf8", "#7dd3fc", "#bae6fd"],
  codex: ["#10a37f", "#34d399", "#6ee7b7", "#a7f3d0"],
  claude: ["#d97757", "#fb923c", "#fdba74", "#fed7aa"],
  cursor: ["#8b5cf6", "#a78bfa", "#c4b5fd", "#ddd6fe"],
  kimi: ["#4338ca", "#6366f1", "#818cf8", "#a5b4fc"],
  gemini: ["#2f6fe0", "#4285F4", "#7aa7f8", "#aec9fb"],
  grok: ["#0f7ecb", "#1d9bf0", "#57b3f4", "#8ccaf8"],
  qoder: ["#5a4bd0", "#6c5ce7", "#8d7ded", "#ae9ff2"],
  opencodego: ["#d96110", "#f97316", "#fb8f47", "#fcaa74"],
  minimax: ["#009657", "#00b96b", "#3fcf90", "#78dcae"],
  moonshot: ["#3833b4", "#4540d6", "#6b66e0", "#918ce9"],
  deepseek: ["#3a53d0", "#4D6BFE", "#7a90fe", "#a6b4fe"],
  longcat: ["#c9a400", "#e6bd00", "#ffd100", "#ffdb4d"],
  mimo: ["#d95700", "#ff6900", "#ff8733", "#ffa466"],
  alibaba: ["#d95700", "#ff6a00", "#ff8833", "#ffa666"],
  alibabatoken: ["#e5772c", "#ff9240", "#ffa866", "#ffbd8c"],
  stepfun: ["#2d68cc", "#3b82f6", "#6ba1f9", "#96bffb"],
  doubao: ["#00a486", "#00d4aa", "#33debb", "#66e6cc"],
};

// 模式 A：label 是品牌名不进词典；description 存词典键，渲染时查（跟随 UI 语言）
export const AGENT_VISIBILITY_OPTIONS: ReadonlyArray<{
  id: AgentId;
  label: string;
  descriptionKey: MessageKey;
}> = [
  {
    id: "zai",
    label: "Z.ai",
    descriptionKey: "settings.agentZaiDesc",
  },
  {
    id: "codex",
    label: "Codex",
    descriptionKey: "settings.agentCodexDesc",
  },
  {
    id: "claude",
    label: "Claude",
    descriptionKey: "settings.agentClaudeDesc",
  },
  {
    id: "cursor",
    label: "Cursor",
    descriptionKey: "settings.agentCursorDesc",
  },
  {
    id: "kimi",
    label: "Kimi",
    descriptionKey: "settings.agentKimiDesc",
  },
  {
    id: "gemini",
    label: "Gemini",
    descriptionKey: "settings.agentGeminiDesc",
  },
  {
    id: "grok",
    label: "Grok",
    descriptionKey: "settings.agentGrokDesc",
  },
  {
    id: "qoder",
    label: "Qoder",
    descriptionKey: "settings.agentQoderDesc",
  },
  {
    id: "opencodego",
    label: "OpenCode",
    descriptionKey: "settings.agentOpencodegoDesc",
  },
  {
    id: "minimax",
    label: "MiniMax",
    descriptionKey: "settings.agentMinimaxDesc",
  },
  {
    id: "moonshot",
    label: "Moonshot",
    descriptionKey: "settings.agentMoonshotDesc",
  },
  {
    id: "deepseek",
    label: "DeepSeek",
    descriptionKey: "settings.agentDeepseekDesc",
  },
  {
    id: "longcat",
    label: "LongCat",
    descriptionKey: "settings.agentLongcatDesc",
  },
  {
    id: "mimo",
    label: "MiMo",
    descriptionKey: "settings.agentMimoDesc",
  },
  {
    id: "alibaba",
    label: "通义灵码",
    descriptionKey: "settings.agentAlibabaDesc",
  },
  {
    id: "alibabatoken",
    label: "百炼Token包",
    descriptionKey: "settings.agentAlibabatokenDesc",
  },
  {
    id: "stepfun",
    label: "StepFun",
    descriptionKey: "settings.agentStepfunDesc",
  },
  {
    id: "doubao",
    label: "火山引擎",
    descriptionKey: "settings.agentDoubaoDesc",
  },
];

/** 凭证驱动 provider 的录入形态：凭证类型（决定添加弹层的输入引导）。
 *  grok 并入本地 ~/.grok/auth.json 读取，手动补充的凭证为访问 Token。 */
export const CREDENTIAL_AGENT_KIND: Record<CredentialAgentId, "apiKey" | "cookie" | "token"> = {
  gemini: "apiKey",
  grok: "token",
  qoder: "cookie",
  opencodego: "apiKey",
  minimax: "apiKey",
  moonshot: "apiKey",
  deepseek: "apiKey",
  longcat: "cookie",
  // MiMo 凭证是浏览器复制的 Cookie（需含 api-platform_serviceToken 与 userId）
  mimo: "cookie",
  alibaba: "apiKey",
  // 阿里 Token Plan（百炼 Token 包）凭证是浏览器复制的控制台 Cookie
  //（Team 订阅摘要与 Personal/Solo 滚动窗口共用，region 分中国站/国际站）
  alibabatoken: "cookie",
  // StepFun 凭证是浏览器复制的 Oasis-Token（platform.stepfun.com 登录态 JWT）
  stepfun: "token",
  doubao: "apiKey",
};

const STORAGE_KEY = "zbar-agent-visibility";

/** 「有凭证自动显示」的显式关闭标记（独立 localStorage 键，向后兼容：
 *  不改 visibility 结构，旧数据无需迁移）。用户在设置页手动关闭某
 *  凭证型 agent 后记录，此后 presence 无→有不再自动开启该 agent；
 *  再次手动开启时清除标记。 */
const MANUALLY_DISABLED_KEY = "zbar-agent-manually-disabled";

type ManuallyDisabledMap = Partial<Record<AgentId, boolean>>;

function loadManuallyDisabled(): ManuallyDisabledMap {
  try {
    const raw = localStorage.getItem(MANUALLY_DISABLED_KEY);
    if (!raw) return {};
    const parsed = JSON.parse(raw) as ManuallyDisabledMap;
    if (parsed && typeof parsed === "object") return parsed;
  } catch {
    // 存储不可用/内容损坏时视为无标记，仅影响自动开启行为
  }
  return {};
}

function saveManuallyDisabled(map: ManuallyDisabledMap): void {
  try {
    localStorage.setItem(MANUALLY_DISABLED_KEY, JSON.stringify(map));
  } catch {
    // 隐私模式或存储配额不足时静默降级（仅丢失「不再自动开启」记忆）
  }
}

/** 设置页手动关闭时记录显式关闭标记（幂等）。 */
export function markAgentManuallyDisabled(id: AgentId): void {
  const map = loadManuallyDisabled();
  if (map[id]) return;
  saveManuallyDisabled({ ...map, [id]: true });
}

/** 手动开启时清除显式关闭标记（此后恢复「有凭证自动显示」联动）。 */
export function clearAgentManuallyDisabled(id: AgentId): void {
  const map = loadManuallyDisabled();
  if (!map[id]) return;
  const next = { ...map };
  delete next[id];
  saveManuallyDisabled(next);
}

/** 该 agent 是否被用户显式关闭过（enableAgentByCredential 的触发前置检查）。 */
export function isAgentManuallyDisabled(id: AgentId): boolean {
  return !!loadManuallyDisabled()[id];
}

/** 首批 5 个 agent 默认开启（升级后行为不变）；新 provider 默认隐藏，
 *  由「有凭证自动开启」或设置页手动开启。 */
const DEFAULT_VISIBILITY: AgentVisibility = {
  zai: true,
  codex: true,
  claude: true,
  cursor: true,
  kimi: true,
  gemini: false,
  grok: false,
  qoder: false,
  opencodego: false,
  minimax: false,
  moonshot: false,
  deepseek: false,
  longcat: false,
  mimo: false,
  alibaba: false,
  alibabatoken: false,
  stepfun: false,
  doubao: false,
};

/** 读取展示偏好；缺失或损坏字段回退默认值（首批 5 个保持「缺省即开」，
 *  新 provider「缺省即关」），保证升级后行为不变。 */
export function loadAgentVisibility(): AgentVisibility {
  const next = { ...DEFAULT_VISIBILITY };
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return next;
    const parsed = JSON.parse(raw) as Partial<Record<AgentId, unknown>>;
    for (const id of Object.keys(DEFAULT_VISIBILITY) as AgentId[]) {
      if (typeof parsed[id] === "boolean") {
        next[id] = parsed[id] as boolean;
      }
    }
  } catch {
    // 存储不可用/内容损坏时全量回退默认值
  }
  return next;
}

/** 设置页切换后立即保存，异常时静默降级为当前会话内状态。 */
export function saveAgentVisibility(visibility: AgentVisibility): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(visibility));
  } catch {
    // 隐私模式或存储配额不足不应阻断设置页操作。
  }
}

// ============================================================
// 跨层联动事件：visibility 状态提升在 App 层，localStorage 写入可能
// 发生在任意组件层（设置页开关 / 凭证添加自动开启），统一用 window
// 自定义事件通知 App 重读——最小侵入，不引入新的状态管理。
// ============================================================

/** visibility 变化广播（enableAgentByCredential 触发，App 监听后重读 localStorage） */
export const AGENT_VISIBILITY_EVENT = "zbar-agent-visibility-changed";

/** 凭证增删改广播（detail = { provider }）：DataCache 刷新凭证缓存、
 *  App 重查 presence 决定是否自动开启对应 agent 的展示。 */
export const CREDENTIALS_CHANGED_EVENT = "zbar-credentials-changed";

/**
 * 「有凭证自动显示」：把该 provider 的 visibility 置 true 并持久化。
 * 仅在当前为 false 时写入（幂等）。用户在设置页显式关闭过的 agent
 * （见 MANUALLY_DISABLED_KEY 标记）不再自动开启——凭证从无到有仅在
 * 用户未表达过「不要它」时才视为需要展示；重新手动开启即恢复联动。
 */
export function enableAgentByCredential(id: AgentId): void {
  if (isAgentManuallyDisabled(id)) return;
  const current = loadAgentVisibility();
  if (current[id]) return;
  saveAgentVisibility({ ...current, [id]: true });
  window.dispatchEvent(new Event(AGENT_VISIBILITY_EVENT));
}

/**
 * 删除某凭证型 agent 的最后一条凭证后回退展示偏好：visibility 置 false
 * 并持久化（与 enableAgentByCredential 对称）。仅在当前为 true 时写入
 * （幂等）；设置页可随时手动重开，不受标记影响。
 */
export function disableAgentByCredential(id: AgentId): void {
  const current = loadAgentVisibility();
  if (!current[id]) return;
  saveAgentVisibility({ ...current, [id]: false });
  window.dispatchEvent(new Event(AGENT_VISIBILITY_EVENT));
}

/** 凭证增删改后广播（CredentialsCard 在操作成功后调用）。 */
export function notifyCredentialsChanged(provider: string): void {
  window.dispatchEvent(
    new CustomEvent(CREDENTIALS_CHANGED_EVENT, { detail: { provider } })
  );
}

// ============================================================
// 数据来源字符串（后端 source 字段："zcode" | "codex" | ...）的展示元数据。
// 项目浏览器等按 source 字符串工作的界面使用，保证与
// AGENT_COLOR / AGENT_VISIBILITY_OPTIONS 的品牌色与名称一致。
// ============================================================

/** 未知来源的兜底展示：中性灰 */
const UNKNOWN_SOURCE_META = { label: "Unknown", color: "#94a3b8" };

const SOURCE_META: Record<string, { label: string; color: string }> = {
  zcode: { label: "Z.ai", color: AGENT_COLOR.zai },
  codex: { label: "Codex", color: AGENT_COLOR.codex },
  claude: { label: "Claude", color: AGENT_COLOR.claude },
  cursor: { label: "Cursor", color: AGENT_COLOR.cursor },
  kimi: { label: "Kimi", color: AGENT_COLOR.kimi },
  gemini: { label: "Gemini", color: AGENT_COLOR.gemini },
  grok: { label: "Grok", color: AGENT_COLOR.grok },
  qoder: { label: "Qoder", color: AGENT_COLOR.qoder },
  opencodego: { label: "OpenCode", color: AGENT_COLOR.opencodego },
  minimax: { label: "MiniMax", color: AGENT_COLOR.minimax },
  moonshot: { label: "Moonshot", color: AGENT_COLOR.moonshot },
  deepseek: { label: "DeepSeek", color: AGENT_COLOR.deepseek },
  longcat: { label: "LongCat", color: AGENT_COLOR.longcat },
  mimo: { label: "MiMo", color: AGENT_COLOR.mimo },
  alibaba: { label: "通义灵码", color: AGENT_COLOR.alibaba },
  alibabatoken: { label: "百炼Token包", color: AGENT_COLOR.alibabatoken },
  stepfun: { label: "StepFun", color: AGENT_COLOR.stepfun },
  doubao: { label: "火山引擎", color: AGENT_COLOR.doubao },
};

/** source 字符串 → 品牌名 + 品牌色（未知来源回退中性灰，不抛错） */
export function sourceMeta(source: string): { label: string; color: string } {
  return SOURCE_META[source] ?? UNKNOWN_SOURCE_META;
}

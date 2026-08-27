/** 统计页的 Agent 展示偏好，仅影响界面展示，不影响本地采集和同步。 */
import type { MessageKey } from "./i18n";

export type AgentId = "zai" | "codex" | "claude" | "cursor" | "kimi";

export type AgentVisibility = Record<AgentId, boolean>;

/** 各 Agent 的品牌展示色（汇总页与对比页共用，保证跨页视觉一致）。 */
export const AGENT_COLOR: Record<AgentId, string> = {
  zai: "#0ea5e9",
  codex: "#10a37f",
  claude: "#d97757",
  cursor: "#8b5cf6",
  kimi: "#4338ca",
};

/** 同一 Agent 的多账号系列用同色系色阶区分（明度递增，4 档，超出取最末档）。
 *  不用透明度方案：细柱上不可辨，且低透明度有"弱化"的语义误导。 */
export const AGENT_COLOR_SCALE: Record<AgentId, readonly string[]> = {
  zai: ["#0ea5e9", "#38bdf8", "#7dd3fc", "#bae6fd"],
  codex: ["#10a37f", "#34d399", "#6ee7b7", "#a7f3d0"],
  claude: ["#d97757", "#fb923c", "#fdba74", "#fed7aa"],
  cursor: ["#8b5cf6", "#a78bfa", "#c4b5fd", "#ddd6fe"],
  kimi: ["#4338ca", "#6366f1", "#818cf8", "#a5b4fc"],
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
];

const STORAGE_KEY = "zbar-agent-visibility";

const DEFAULT_VISIBILITY: AgentVisibility = {
  zai: true,
  codex: true,
  claude: true,
  cursor: true,
  kimi: true,
};

/** 读取展示偏好；缺失或损坏字段默认开启，保证升级后行为不变。 */
export function loadAgentVisibility(): AgentVisibility {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_VISIBILITY };
    const parsed = JSON.parse(raw) as Partial<Record<AgentId, unknown>>;
    return {
      zai: parsed.zai !== false,
      codex: parsed.codex !== false,
      claude: parsed.claude !== false,
      cursor: parsed.cursor !== false,
      kimi: parsed.kimi !== false,
    };
  } catch {
    return { ...DEFAULT_VISIBILITY };
  }
}

/** 设置页切换后立即保存，异常时静默降级为当前会话内状态。 */
export function saveAgentVisibility(visibility: AgentVisibility): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(visibility));
  } catch {
    // 隐私模式或存储配额不足不应阻断设置页操作。
  }
}

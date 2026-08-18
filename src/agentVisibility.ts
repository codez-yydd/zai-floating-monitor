/** 统计页的 Agent 展示偏好，仅影响界面展示，不影响本地采集和同步。 */
import type { MessageKey } from "./i18n";

export type AgentId = "zai" | "codex" | "claude" | "cursor";

export type AgentVisibility = Record<AgentId, boolean>;

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
];

const STORAGE_KEY = "zbar-agent-visibility";

const DEFAULT_VISIBILITY: AgentVisibility = {
  zai: true,
  codex: true,
  claude: true,
  cursor: true,
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

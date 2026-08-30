/**
 * 添加服务浮层：StatsPanel tab 栏「＋」按钮的下拉服务选择卡。
 *
 * 解决「新服务的 tab 默认隐藏（有凭证才显示）→ 新用户没有添加第一个
 * 凭证的入口」的死循环：浮层列出全部凭证型服务（排除 gemini/opencodego
 * 本地直读型），点选后由 StatsPanel 弹出该服务的添加凭证表单（复用
 * CredentialsCard 的 CredentialFormDialog），添加成功后 tab 自动出现并切换。
 *
 * 本组件为纯展示浮层：外点关闭与 Esc 关闭由 StatsPanel 的锚容器负责
 * （「＋」按钮需包含在点击豁免范围内，保证再次点击按钮可正常切换开关）。
 */
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import { useDataCache } from "./DataCache";
import {
  AGENT_VISIBILITY_OPTIONS,
  CREDENTIAL_AGENTS,
  CREDENTIAL_AGENT_KIND,
  isLocalAgent,
  type CredentialAgentId,
} from "./agentVisibility";
import { KIND_BADGE } from "./CredentialsCard";
import { NOT_YET_PROVIDERS } from "./GenericQuotaPanel";
import { BrandIcon } from "./BrandIcon";

/** 设置页「添加凭证」快捷入口 → StatsPanel 的事件名（detail = provider id）。
 *  App 监听后切回统计页并把目标 provider 交给 StatsPanel 打开添加表单。 */
export const ADD_SERVICE_EVENT = "zbar-open-credential-add";

/** 可添加凭证的服务清单：凭证型 provider 排除本地直读型（无凭证可录） */
const ADDABLE_SERVICES: readonly CredentialAgentId[] = CREDENTIAL_AGENTS.filter(
  (id) => !isLocalAgent(id)
);

interface AddServiceMenuProps {
  /** 点选某个服务（StatsPanel 关闭浮层并弹出该服务的添加表单） */
  onPick: (provider: CredentialAgentId) => void;
}

/** 服务选择下拉卡：每行 = 品牌图标 + 品牌名 + 凭证类型徽章 + 已有凭证数 +
 *  简短说明；未接入查询的服务（doubao）加「查询即将上线」标记。列表超一屏
 *  可滚动；已有凭证的服务同样可点（支持继续添加多账号）。 */
export function AddServiceMenu({ onPick }: AddServiceMenuProps) {
  const { t } = useI18n();
  // 已有凭证数（掩码元数据缓存；未加载的 provider 不显示计数，不在此处
  // 主动拉取——避免浮层打开时对 11 个 provider 并发读文件）
  const { credentials } = useDataCache();

  return (
    <div
      className="absolute right-0 top-full mt-1 z-30 w-56 rounded-lg bg-elevated border border-slate-900/10 shadow-xl p-1.5"
      role="menu"
      aria-label={t("credentials.addServiceTitle")}
    >
      <div className="text-[11px] font-semibold text-slate-900 px-1 pt-0.5">
        {t("credentials.addServiceTitle")}
      </div>
      <p className="text-[9px] text-slate-500 leading-relaxed px-1 pb-1 pt-0.5">
        {t("credentials.addServiceHint")}
      </p>
      <div className="max-h-64 overflow-y-auto overscroll-contain space-y-0.5">
        {ADDABLE_SERVICES.map((id) => {
          const label =
            AGENT_VISIBILITY_OPTIONS.find((o) => o.id === id)?.label ?? id;
          const kindBadge = KIND_BADGE[CREDENTIAL_AGENT_KIND[id]];
          const count = credentials[id]?.length ?? 0;
          return (
            <button
              key={id}
              type="button"
              role="menuitem"
              onClick={() => onPick(id)}
              className="w-full flex items-start gap-1.5 rounded-md px-1.5 py-1.5 text-left hover:bg-sky-500/10 transition-colors"
            >
              <BrandIcon brand={id} className="h-4 w-4 shrink-0 mt-0.5" />
              <span className="min-w-0 flex-1">
                <span className="flex items-center gap-1 flex-wrap">
                  <span className="text-[10px] font-medium text-slate-900/85">
                    {label}
                  </span>
                  <span
                    className={`shrink-0 px-1 py-px rounded text-[8px] font-medium ${kindBadge.cls}`}
                  >
                    {t(kindBadge.key)}
                  </span>
                  {count > 0 && (
                    <span className="num shrink-0 px-1 py-px rounded text-[8px] bg-emerald-500/12 text-emerald-700">
                      {t("credentials.entriesCount", { n: count })}
                    </span>
                  )}
                  {NOT_YET_PROVIDERS.includes(id) && (
                    <span className="shrink-0 px-1 py-px rounded text-[8px] font-medium bg-amber-500/12 text-amber-700">
                      {t("credentials.comingSoon")}
                    </span>
                  )}
                </span>
                <span className="block text-[9px] text-slate-500 leading-relaxed mt-0.5 line-clamp-2">
                  {t(`credentials.guideBrief.${id}` as MessageKey)}
                </span>
              </span>
            </button>
          );
        })}
      </div>
    </div>
  );
}

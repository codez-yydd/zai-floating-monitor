import { useEffect, useState } from "react";
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import { useDataCache } from "./DataCache";
import type { CredentialKind, ProviderQuotaEntry } from "./types";
import {
  AGENT_COLOR,
  CREDENTIAL_AGENT_KIND,
  isLocalAgent,
  type CredentialAgentId,
} from "./agentVisibility";
import { CredentialsCard, type CredentialRegionOption } from "./CredentialsCard";
import { BrandIcon } from "./BrandIcon";
import { QuotaEntryCard } from "./QuotaEntryCard";
import { useResetDisplay } from "./resetDisplay";

interface GenericQuotaPanelProps {
  /** provider 标识（决定凭证文件与引导文案键） */
  provider: CredentialAgentId;
  /** 面板标题（品牌名，来自 AGENT_VISIBILITY_OPTIONS） */
  title: string;
  /** 该 provider 的凭证类型（默认取 CREDENTIAL_AGENT_KIND，可显式覆盖） */
  kind?: CredentialKind;
  /** 品牌色（进度条着色；默认 AGENT_COLOR） */
  color?: string;
  /** 区域选项（仅区分国内/国际站的 provider 传入） */
  regionOptions?: ReadonlyArray<CredentialRegionOption>;
}

/** 「接入即将上线」只对这些前端预留的 provider 显示（后端无查询分支，
 *  返回空数组即"确认未接入"）；其余已接入 provider 空结果一律按
 *  查询中/失败表达，不误导为未接入。
 *  导出供添加服务浮层（AddServiceMenu）给对应行加「查询即将上线」标记。 */
export const NOT_YET_PROVIDERS: readonly string[] = ["doubao"];

/**
 * 通用额度面板：顶部凭证管理卡 + 每条凭证一张配额卡 + 面板级手动刷新。
 * 数据从 DataCache 的 providerQuota 缓存读取（后端 get_provider_quota 按
 * provider 分发，把各服务接口响应映射为 ProviderQuotaEntry[]，本组件不感知
 * 具体接口差异）。凭证未就绪时的展示门槛由 StatsPanel 的 tab 逻辑控制，
 * 本面板只在「已进入 tab」后渲染：无凭证 → 凭证卡引导；有凭证 → 查询中/
 * 未接入提示或配额卡。
 */
export function GenericQuotaPanel({
  provider,
  title,
  kind,
  color,
  regionOptions,
}: GenericQuotaPanelProps) {
  const { t } = useI18n();
  const resetDisplay = useResetDisplay();
  const { credentials, providerQuota, refreshProviderQuota } = useDataCache();
  const cacheEntry = providerQuota[provider];
  const quotaEntries: ProviderQuotaEntry[] = cacheEntry?.entries ?? [];
  const refreshing = cacheEntry?.refreshing ?? false;
  // 是否已完成首轮查询（ts>0）：区分「还没查过」与「查过确认无数据」
  const firstRoundDone = (cacheEntry?.ts ?? 0) > 0;
  const hasCreds = (credentials[provider]?.length ?? 0) > 0;
  const accent = color ?? AGENT_COLOR[provider];
  const guideKey = `credentials.guide.${provider}` as MessageKey;

  // 有凭证但额度缓存未就绪（首次添加 / 冷启动）→ 立即补刷一轮，
  // 不等 120s 通用轮询（照 ClaudeOtherAccounts 先例）
  useEffect(() => {
    if (hasCreds && !providerQuota[provider]) {
      void refreshProviderQuota(provider);
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [hasCreds]);

  // 倒计时显示用时钟（有窗口带重置时间才有意义，空数据不启定时器）
  const hasResets = quotaEntries.some((e) =>
    e.windows.some((w) => w.resetsAt != null)
  );
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    if (!hasResets) return;
    const timer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [hasResets]);

  return (
    <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 page-stack">
      {/* 面板头：品牌 + 标题 + 手动刷新（查询中禁用转灰，照 StatsPanel 先例） */}
      <div className="flex items-center justify-between gap-2 -mb-1">
        <div className="flex items-center gap-1.5 min-w-0">
          <BrandIcon brand={provider} className="h-3.5 w-3.5 shrink-0" />
          <span className="section-title truncate">{title}</span>
        </div>
        <button
          onClick={() => void refreshProviderQuota(provider)}
          disabled={refreshing}
          className={`toolbar-btn shrink-0 ${refreshing ? "opacity-40" : ""}`}
          title={t("common.refresh")}
        >
          ↻
        </button>
      </div>

      <CredentialsCard
        provider={provider}
        kind={kind ?? CREDENTIAL_AGENT_KIND[provider]}
        guideKey={guideKey}
        brand={provider}
        regionOptions={regionOptions}
        // 本地型 provider（opencodego/gemini）数据来自本地库，凭证为可选，
        // 空态按「无需凭证」引导，且不提供添加入口（手动凭证不参与查询）
        optional={isLocalAgent(provider)}
      />

      {/* 额度空态三档：无凭证（凭证卡空态已含引导，不重复提示）/
          有凭证查询中（refreshing 或未完成首轮）/ 确认未接入
          （doubao 等后端暂无查询分支的 provider，首轮返回空数组） */}
      {quotaEntries.length === 0 && hasCreds && (!firstRoundDone || refreshing || NOT_YET_PROVIDERS.includes(provider)) && (
        <div className="card-base rounded-2xl px-3 py-3">
          <p className="text-[10px] text-slate-500 leading-relaxed">
            {refreshing || !firstRoundDone
              ? t("credentials.quotaRefreshing")
              : NOT_YET_PROVIDERS.includes(provider)
                ? t("credentials.quotaPending")
                : null}
          </p>
        </div>
      )}

      {/* 每条凭证一张配额卡（共享组件，Claude 面板「其他账号」区复用） */}
      {quotaEntries.map((entry) => (
        <QuotaEntryCard
          key={entry.credentialId}
          entry={entry}
          accent={accent}
          resetDisplay={resetDisplay}
          now={now}
        />
      ))}
    </div>
  );
}

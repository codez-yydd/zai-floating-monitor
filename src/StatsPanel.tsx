import { useEffect, useMemo, useRef, useState } from "react";
import { UPDATE_READY_KEY, UPDATE_READY_EVENT } from "./updater";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { CredentialKind, Currency, PricingConfig, StatsTab } from "./types";
import { fetchPin, setPin } from "./api";
import { useDataCache } from "./DataCache";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker } from "./RangePicker";
import { ZaiStatsContent } from "./ZaiStatsContent";
import { CodexPanel } from "./CodexPanel";
import { ClaudePanel } from "./ClaudePanel";
import { CursorPanel } from "./CursorPanel";
import { KimiPanel } from "./KimiPanel";
import { SummaryTab } from "./SummaryTab";
import { ProjectsPanel } from "./ProjectsPanel";
import { GenericQuotaPanel } from "./GenericQuotaPanel";
import { AddServiceMenu } from "./AddServiceMenu";
import {
  CredentialFormDialog,
  type CredentialOAuthFlow,
  type CredentialRegionOption,
} from "./CredentialsCard";
import {
  addProviderCredential,
  pollKimiDeviceAuth,
  startKimiDeviceAuth,
} from "./api";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  AGENT_VISIBILITY_OPTIONS,
  CREDENTIAL_AGENT_KIND,
  enableAgentByCredential,
  isCredentialAgent,
  isPurePreferenceAgent,
  notifyCredentialsChanged,
  type AgentVisibility,
  type CredentialAgentId,
} from "./agentVisibility";
import { LanguageToggle, ThemeToggle } from "./layout";
import { useI18n, type TFn } from "./i18n";
import { dateLocale } from "./i18n/locale";

/** 区分国内/国际站的 provider：录入凭证时提供 region 下拉（与后端
 *  host_for_region 的站点路由一致，Cookie/Key 归属站点必须匹配）。
 *  kimi 同为双站（区域决定 usages 与 OAuth 域名组）。 */
const REGION_PROVIDERS: readonly string[] = [
  "kimi",
  "qoder",
  "alibaba",
  "alibabatoken",
  "minimax",
  "moonshot",
];

/** provider → region 下拉选项（无区域概念的服务返回 undefined，弹层不显示）。
 *  tab 面板与「＋添加服务」表单共用同一份配置。 */
function regionOptionsFor(
  provider: string,
  t: TFn
): ReadonlyArray<CredentialRegionOption> | undefined {
  if (!REGION_PROVIDERS.includes(provider)) return undefined;
  return [
    { value: "cn", label: t("credentials.regionCn") },
    { value: "global", label: t("credentials.regionGlobal") },
  ];
}

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
  /** 凭证驱动 provider 的「已有凭证」状态（provider → bool；App 启动/事件时批量刷新） */
  credentialPresence: Record<string, boolean>;
  onGoPricing: () => void;
  onGoSync: () => void;
  onGoReports: () => void;
  onGoTheme: () => void;
  onGoSettings: () => void;
  /** 设置页「添加凭证」快捷入口跳转：初始定位到该 provider 并直接打开
   *  添加表单（App 暂存 pending provider 传入，挂载即消费）。 */
  initialAdd?: string | null;
  /** initialAdd 已消费的回调（App 清空 pending，避免下次挂载重复打开） */
  onInitialAddConsumed?: () => void;
}

function loadStatsTab(agentVisibility: AgentVisibility): StatsTab {
  try {
    const saved = localStorage.getItem("zbar-tab");
    if (saved === "summary") return saved;
    if (saved === "projects") return saved;
    // Agent tab：任一 AgentId 均合法（含新 provider），按展示偏好恢复或回汇总
    if (saved && saved in agentVisibility) {
      return agentVisibility[saved as keyof AgentVisibility]
        ? (saved as StatsTab)
        : "summary";
    }
  } catch {
    // 存储不可用时使用默认标签，不阻断统计面板启动。
  }
  return "summary";
}

/**
 * 统计面板 —— 纯展示层。
 *
 * 所有数据（z.ai / Cursor / quota）由全局 DataProvider 统一预加载 + 定时刷新，
 * 此组件仅负责从缓存读取并渲染，不再自己发请求或维护定时器。
 * 数据常驻 Provider，切到其他页面再切回来瞬时恢复，无需重新加载。
 */
export function StatsPanel({
  currency,
  pricing,
  agentVisibility,
  credentialPresence,
  onGoPricing,
  onGoSync,
  onGoReports,
  onGoTheme,
  onGoSettings,
  initialAdd,
  onInitialAddConsumed,
}: Props) {
  const cache = useDataCache();
  const { locale, t } = useI18n();
  const {
    preset,
    custom,
    trendBucket,
    deviceFilter,
    setPreset,
    setCustom,
    setDeviceFilter,
    stats,
    cost,
    trend,
    error,
    codex,
    codexError,
    claude,
    claudeError,
    cursor,
    cursorError,
    kimi,
    kimiError,
    fxRate,
    syncConfig,
    remoteDevices,
    syncEnabled,
    agentQuotaDeltas,
    lastUpdate,
    refreshing,
    refresh,
  } = cache;

  // 标签：汇总 | Z.ai | Codex | Claude | Cursor（关闭的 Agent 不显示）
  // 设置页快捷入口跳转时初始 tab 直接定位到目标 provider（避免先落汇总再跳）
  const [tab, setTab] = useState<StatsTab>(() =>
    initialAdd && isCredentialAgent(initialAdd)
      ? initialAdd
      : loadStatsTab(agentVisibility)
  );

  // ===== 「＋添加服务」入口 =====
  // addMenuOpen：服务选择浮层；addProvider：添加目标（非空时弹出该服务的
  // 添加表单）。设置页快捷入口跳转时直接进入添加态（同一套提交链路）。
  const [addMenuOpen, setAddMenuOpen] = useState(false);
  const [addProvider, setAddProvider] = useState<CredentialAgentId | null>(() =>
    initialAdd && isCredentialAgent(initialAdd) ? initialAdd : null
  );
  const [addBusy, setAddBusy] = useState(false);
  const [addError, setAddError] = useState<string | null>(null);
  // 「＋」按钮 + 浮层的锚容器（点击外部关闭时把按钮一并豁免，
  // 保证再次点击「＋」可正常切换浮层开关）
  const addAnchorRef = useRef<HTMLDivElement | null>(null);
  // 设置页快捷入口跳转的锚定目标（仅挂载时取值）：与 addProvider 的临时
  // 保留分开——用户主动进入该服务面板，取消自动弹出的表单后仍应停留在
  // 面板内浏览凭证卡，而不是被门槛 effect 弹回汇总页
  const pinnedProviderRef = useRef<CredentialAgentId | null>(
    initialAdd && isCredentialAgent(initialAdd) ? initialAdd : null
  );

  // initialAdd 只在挂载时消费一次：App 收到回调即清空 pending
  useEffect(() => {
    if (initialAdd && isCredentialAgent(initialAdd)) {
      onInitialAddConsumed?.();
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 浮层交互：点击外部 / Esc 关闭（浮层挂载时才注册）
  useEffect(() => {
    if (!addMenuOpen) return;
    const onDown = (e: MouseEvent) => {
      if (
        addAnchorRef.current &&
        e.target instanceof Node &&
        !addAnchorRef.current.contains(e.target)
      ) {
        setAddMenuOpen(false);
      }
    };
    const onKey = (e: KeyboardEvent) => {
      if (e.key === "Escape") setAddMenuOpen(false);
    };
    document.addEventListener("mousedown", onDown, true);
    window.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDown, true);
      window.removeEventListener("keydown", onKey);
    };
  }, [addMenuOpen]);

  // 添加表单提交（复用 CredentialsCard 添加路径的联动语义）：保存成功 →
  // 开启 tab 偏好（有凭证自动显示，含 manually-disabled 标记检查）→
  // 广播 presence 重探 → 切到该服务 tab → 关闭表单
  const handleAddSubmit = async (form: {
    label: string;
    secret: string;
    region: string | null;
    kind?: CredentialKind;
  }) => {
    if (!addProvider) return;
    setAddBusy(true);
    setAddError(null);
    const provider = addProvider;
    try {
      // 无区域下拉的服务 region 初始为空串，原样提交会被后端判为非法
      // 区域；空串/纯空白统一提交 null（未选择），对齐 CredentialsCard。
      // kind：kimi 表单内可选（OAuth 令牌/API Key），其余服务用默认映射
      const trimmedRegion = form.region?.trim() ?? "";
      await addProviderCredential(
        provider,
        form.label,
        form.kind ?? CREDENTIAL_AGENT_KIND[provider],
        form.secret,
        trimmedRegion.length > 0 ? trimmedRegion : null
      );
      enableAgentByCredential(provider);
      notifyCredentialsChanged(provider);
      setAddProvider(null);
      setAddError(null);
      setTab(provider);
    } catch (e) {
      setAddError(t("credentials.saveFail", { msg: String(e) }));
    } finally {
      setAddBusy(false);
    }
  };

  // OAuth 网页登录流程（目前仅 kimi 提供）：与手动添加成功后的联动语义
  // 一致——开启 tab 偏好 + 广播 presence/额度刷新 + 切到该服务 tab。
  // useMemo 稳定引用：api 函数为模块级导入（引用恒定），仅 addProvider
  // 变化时重建——弹层每秒重渲染（时钟等）不得让轮询定时器被反复清除重建
  const oauthFlow: CredentialOAuthFlow | undefined = useMemo(
    () =>
      addProvider === "kimi"
        ? {
            onStart: (region) => startKimiDeviceAuth(region),
            onPoll: (sessionId) => pollKimiDeviceAuth(sessionId),
          }
        : undefined,
    [addProvider]
  );
  const handleOAuthSuccess = (provider: CredentialAgentId) => {
    enableAgentByCredential(provider);
    notifyCredentialsChanged(provider);
    setAddProvider(null);
    setAddError(null);
    setTab(provider);
  };

  useEffect(() => {
    if (
      tab !== "summary" &&
      tab !== "projects" &&
      !agentVisibility[tab] &&
      // 凭证驱动的新 provider：有凭证时即使偏好未开也保留（「有凭证自动显示」）；
      // kimi 属首批 5 个（纯偏好控制，见 PURE_PREFERENCE_AGENTS），不参与该保留
      !(isCredentialAgent(tab) && !isPurePreferenceAgent(tab) && credentialPresence[tab]) &&
      // 「＋添加服务」流程中临时保留目标 tab（添加成功前 visibility/presence
      // 均未就绪；取消添加后无此保留，tab 回落汇总——本来就没有该 tab）；
      // 设置页跳转的锚定目标同样保留（用户主动进入该面板）
      tab !== addProvider && tab !== pinnedProviderRef.current
    ) {
      setTab("summary");
    }
  }, [agentVisibility, credentialPresence, tab, addProvider]);

  // 模式 C：「汇总」/「项目」标签走词典（其余是品牌名），语言切换时随 t 重建
  const statTabs = useMemo<
    ReadonlyArray<{ id: StatsTab; label: string; brand?: BrandIconName }>
  >(
    () => [
      { id: "summary", label: t("stats.tab.summary") },
      { id: "projects", label: t("projects.tab") },
      ...AGENT_VISIBILITY_OPTIONS.map((agent) => ({
        id: agent.id,
        label: agent.label,
        brand: agent.id as BrandIconName,
      })),
    ],
    [t]
  );

  const visibleTabs = statTabs.filter((item) =>
    item.id === "summary" || item.id === "projects"
      ? true
      : // 凭证驱动的新 provider：「已启用或有凭证」才显示 tab（默认隐藏，
        // 添加凭证 / 手动开启后出现）；kimi 属首批 5 个，tab 仍纯偏好控制
        //（presence 只驱动「其他账号」区的补刷/轮询，见 PURE_PREFERENCE_AGENTS）
        isCredentialAgent(item.id) && !isPurePreferenceAgent(item.id)
        ? agentVisibility[item.id] || credentialPresence[item.id]
        : agentVisibility[item.id]
  );

  // tab 栏溢出检测：横向可滚动时右缘渐隐（暗示还有更多 tab，可拖动查看）
  const tabScrollRef = useRef<HTMLDivElement | null>(null);
  const [tabOverflow, setTabOverflow] = useState(false);
  useEffect(() => {
    const el = tabScrollRef.current;
    if (!el) return;
    const check = () => setTabOverflow(el.scrollWidth > el.clientWidth + 1);
    check();
    // 容器尺寸变化（窗口缩放 / 字号档位）与 tab 增减都会改变溢出状态
    const observer = new ResizeObserver(check);
    observer.observe(el);
    return () => observer.disconnect();
  }, [visibleTabs.length]);

  // ===== 窗口置顶常驻（仅 Windows）=====
  const isWindows =
    typeof navigator !== "undefined" &&
    /windows/i.test(navigator.userAgent);
  const [pinned, setPinned] = useState(false);

  useEffect(() => {
    if (!isWindows) return;
    fetchPin()
      .then(setPinned)
      .catch(() => {});
  }, [isWindows]);

  // ===== 更新红点：后台下载完成后，设置入口按钮提示 =====
  const [updateReady, setUpdateReady] = useState(() => {
    try {
      return Boolean(localStorage.getItem(UPDATE_READY_KEY));
    } catch {
      return false;
    }
  });
  useEffect(() => {
    const sync = () => {
      try {
        setUpdateReady(Boolean(localStorage.getItem(UPDATE_READY_KEY)));
      } catch {
        /* 忽略存储异常 */
      }
    };
    window.addEventListener(UPDATE_READY_EVENT, sync);
    return () => window.removeEventListener(UPDATE_READY_EVENT, sync);
  }, []);

  // 记忆当前标签（localStorage 异常静默，对齐 cache.ts：记忆仅锦上添花，不影响主流程）
  useEffect(() => {
    try {
      localStorage.setItem("zbar-tab", tab);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等 */
    }
  }, [tab]);

  return (
    // relative：承载「＋添加服务」的添加凭证全卡弹层（CredentialFormDialog
    // 的 absolute inset-0 以最近定位祖先为界）
    <div className="relative flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3 pt-2.5 pb-2 border-b border-slate-900/8">
        {/* Windows 无边框窗口拖动 */}
        <div
          className={`flex items-center justify-between mb-2 ${
            isWindows ? "cursor-default" : ""
          }`}
          onMouseDown={
            isWindows
              ? (e) => {
                  if (!(e.target as HTMLElement).closest("button")) {
                    getCurrentWindow().startDragging();
                  }
                }
              : undefined
          }
        >
          <div className="flex items-center gap-1.5 select-none">
            <h1 className="text-[13px] font-bold text-slate-900/90 tracking-tight">
              ZCode Token
            </h1>
          </div>
          <div className="flex items-center gap-0.5">
            <button
              onClick={onGoReports}
              className="toolbar-btn"
              title={t("stats.reports")}
            >
              📊
            </button>
            <button
              onClick={onGoSync}
              className={`toolbar-btn ${
                syncEnabled ? "text-emerald-600!" : ""
              }`}
              title={syncEnabled ? t("stats.syncOn") : t("stats.syncOff")}
            >
              ⇅
            </button>
            <ThemeToggle />
            <LanguageToggle />
            <button
              onClick={onGoTheme}
              className="toolbar-btn"
              title={t("theme.toolbarEntry")}
            >
              <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" className="h-3.5 w-3.5" aria-hidden>
                <rect x="3" y="3" width="18" height="18" rx="2" ry="2" />
                <circle cx="9" cy="9" r="2" />
                <path d="m21 15-3.1-3.1a2 2 0 0 0-2.83 0L6 21" />
              </svg>
            </button>
            <button
              onClick={onGoSettings}
              className="toolbar-btn relative"
              title={t("stats.settings")}
            >
              ⚙
              {updateReady && (
                <span className="absolute top-0 right-0 w-1.5 h-1.5 rounded-full bg-rose-500" />
              )}
            </button>
            {isWindows && (
              <button
                onClick={() => {
                  const next = !pinned;
                  setPinned(next);
                  setPin(next).catch(() => setPinned(!next));
                }}
                className={`toolbar-btn ${pinned ? "text-sky-600!" : ""}`}
                title={pinned ? t("stats.unpin") : t("stats.pin")}
              >
                📌
              </button>
            )}
            <button
              onClick={refresh}
              disabled={refreshing}
              className={`toolbar-btn ${refreshing ? "opacity-40" : ""}`}
              title={t("common.refresh")}
            >
              ↻
            </button>
          </div>
        </div>
        <RangePicker
          preset={preset}
          custom={custom}
          onChange={(p, c) => {
            setPreset(p);
            setCustom(c);
          }}
        />
        {/* 设备筛选单独占一行，标签保持完整名称；未来新增 Agent 时横向滚动。 */}
        {syncEnabled && (
          <div className="mt-2 flex items-center">
            <select
              value={deviceFilter}
              onChange={(e) => setDeviceFilter(e.target.value)}
              title={t("stats.deviceFilter")}
              className="num w-[4.75rem] shrink-0 px-1 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60"
            >
              <option value="all">{t("stats.deviceAll")}</option>
              <option value="local">
                {syncConfig?.device_name
                  ? t("stats.deviceLocalName", { name: syncConfig.device_name })
                  : t("stats.deviceLocal")}
              </option>
              {remoteDevices
                .filter((d) => d.device_id !== syncConfig?.device_id)
                .map((d) => (
                  <option key={d.device_id} value={d.device_id}>
                    {t("common.deviceOption", {
                      name: d.device_name,
                      id: d.device_id.slice(0, 6),
                    })}
                  </option>
                ))}
            </select>
          </div>
        )}
        {/* tab 栏：横向滚动区 + 右侧固定「＋添加服务」入口。渐隐遮罩只盖
            滚动区；「＋」在遮罩之外始终可见——新 provider 的 tab 默认隐藏，
            这是新用户添加第一个凭证服务的唯一常驻入口 */}
        <div className={`${syncEnabled ? "mt-1.5" : "mt-2"} flex items-stretch gap-1 min-w-0`}>
          <div
            ref={tabScrollRef}
            className={`min-w-0 overflow-x-auto ${
              tabOverflow ? "tab-fade-right" : ""
            }`}
            aria-label={t("stats.sourcesAria")}
          >
            <div className="flex w-max min-w-full gap-1 p-0.5 rounded-xl bg-slate-900/4">
              {visibleTabs.map((item) => {
              const tabId = item.id;
              const activeColors: Record<string, string> = {
                cursor: "bg-violet-500/12 text-violet-700 shadow-sm",
                claude: "bg-orange-500/12 text-orange-700 shadow-sm",
                codex: "bg-emerald-500/12 text-emerald-700 shadow-sm",
                kimi: "bg-indigo-500/12 text-indigo-700 shadow-sm",
                zai: "bg-sky-500/12 text-sky-700 shadow-sm",
                summary: "bg-sky-500/15 text-sky-700 shadow-sm",
                projects: "bg-amber-500/12 text-amber-700 shadow-sm",
                // 凭证驱动的新 provider：激活色贴近各自品牌色
                gemini: "bg-blue-500/12 text-blue-700 shadow-sm",
                grok: "bg-sky-500/12 text-sky-700 shadow-sm",
                qoder: "bg-violet-500/12 text-violet-700 shadow-sm",
                opencodego: "bg-orange-500/12 text-orange-700 shadow-sm",
                minimax: "bg-emerald-500/12 text-emerald-700 shadow-sm",
                moonshot: "bg-indigo-500/12 text-indigo-700 shadow-sm",
                deepseek: "bg-blue-600/12 text-blue-700 shadow-sm",
                longcat: "bg-yellow-400/20 text-yellow-700 shadow-sm",
                mimo: "bg-orange-500/12 text-orange-700 shadow-sm",
                alibaba: "bg-orange-600/12 text-orange-700 shadow-sm",
                alibabatoken: "bg-orange-400/15 text-orange-700 shadow-sm",
                stepfun: "bg-blue-500/12 text-blue-700 shadow-sm",
                doubao: "bg-teal-500/12 text-teal-700 shadow-sm",
              };
              return (
                <button
                  key={tabId}
                  onClick={() => setTab(tabId)}
                  type="button"
                  aria-pressed={tab === tabId}
                  className={`flex shrink-0 items-center justify-center gap-1 whitespace-nowrap rounded-lg px-2.5 py-1 text-[10px] font-medium transition-all duration-150 ${
                    tab === tabId
                      ? activeColors[tabId] ?? "bg-surface text-slate-900 shadow-sm"
                      : "text-slate-600/60 hover:text-slate-800 hover:bg-slate-900/4"
                  }`}
                >
                  {item.brand && (
                    <BrandIcon brand={item.brand} className="h-3 w-3 shrink-0" />
                  )}
                  <span>{item.label}</span>
                </button>
              );
            })}
            </div>
          </div>
          {/* 「＋添加服务」：锚容器含按钮本体（点击外部关闭时豁免按钮，
              再次点击「＋」可切换浮层）；浮层 absolute 挂在按钮下方 */}
          <div ref={addAnchorRef} className="relative shrink-0">
            <button
              type="button"
              onClick={() => setAddMenuOpen((v) => !v)}
              title={t("credentials.addService")}
              aria-label={t("credentials.addService")}
              aria-expanded={addMenuOpen}
              className="flex h-full w-7 items-center justify-center rounded-xl bg-slate-900/4 text-slate-600/60 hover:text-slate-800 hover:bg-slate-900/8 transition-all duration-150"
            >
              <svg
                viewBox="0 0 24 24"
                fill="none"
                stroke="currentColor"
                strokeWidth="2.5"
                strokeLinecap="round"
                className="h-3 w-3"
                aria-hidden
              >
                <path d="M12 5v14M5 12h14" />
              </svg>
            </button>
            {addMenuOpen && (
              <AddServiceMenu
                onPick={(p) => {
                  setAddMenuOpen(false);
                  setAddError(null);
                  setAddProvider(p);
                }}
              />
            )}
          </div>
        </div>
      </div>

      {/* 标签内容（zai：额度卡与统计内容同处一个滚动流，额度卡不再固定置顶
          挤压统计区高度，向下滚动时随内容滚出视野。
          额度采样由 DataProvider 全局定时器负责，与组件挂载无关。 */}
      {tab === "zai" ? (
        <div className="flex-1 min-h-0 overflow-y-auto px-3 py-2.5 page-stack">
          <QuotaPanel />
          {error && (
            <div className="mb-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
              {error}
            </div>
          )}
          <ZaiStatsContent
            stats={stats}
            cost={cost}
            trend={trend}
            pricing={pricing}
            currency={currency}
            trendBucket={trendBucket}
          />
        </div>
      ) : tab === "codex" ? (
        <CodexPanel
          snapshot={codex}
          loading={!codex && !codexError}
          error={codexError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.codex?.weekly}
        />
      ) : tab === "claude" ? (
        <ClaudePanel
          snapshot={claude}
          loading={!claude && !claudeError}
          error={claudeError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.claude?.weekly}
        />
      ) : tab === "cursor" ? (
        <CursorPanel
          snapshot={cursor}
          loading={!cursor && !cursorError}
          error={cursorError}
          currency={currency}
          fxRate={fxRate}
          autoQuotaDelta={agentQuotaDeltas.cursor?.cursor_auto}
          apiQuotaDelta={agentQuotaDeltas.cursor?.cursor_api}
        />
      ) : tab === "kimi" ? (
        <KimiPanel
          snapshot={kimi}
          loading={!kimi && !kimiError}
          error={kimiError}
          currency={currency}
          fxRate={fxRate}
          trendBucket={trendBucket}
          pricing={pricing}
          agentQuotaDelta={agentQuotaDeltas.kimi?.weekly}
        />
      ) : isCredentialAgent(tab) ? (
        // 凭证驱动的新 provider：通用额度面板（凭证卡 + 配额卡；配额数据
        // 由面板经 DataCache 的 providerQuota 缓存自取，未接入的 provider
        // entries 为空，仅显示凭证管理与接入提示）
        <GenericQuotaPanel
          provider={tab}
          title={
            AGENT_VISIBILITY_OPTIONS.find((o) => o.id === tab)?.label ?? tab
          }
          // Qoder 区分国际站（qoder.com）与中国站（qoder.com.cn），
          // Cookie 归属站点必须与 region 一致，录入时显式可选；
          // 阿里 Coding Plan / Token Plan、MiniMax / Moonshot 同为
          // 国内/国际双站（选项配置见 regionOptionsFor，与「＋添加服务」
          // 表单共用同一份；后端 host_for_region 按 region 选查询主机）
          regionOptions={regionOptionsFor(tab, t)}
        />
      ) : tab === "projects" ? (
        <ProjectsPanel
          preset={preset}
          custom={custom}
          currency={currency}
          fxRate={fxRate}
        />
      ) : (
        <SummaryTab
          stats={stats}
          cost={cost}
          trend={trend}
          codex={codex}
          claude={claude}
          cursor={cursor}
          kimi={kimi}
          currency={currency}
          bucket={trendBucket}
          fxRate={fxRate}
          pricing={pricing}
          agentVisibility={agentVisibility}
        />
      )}

      {/* 底部 */}
      <div className="px-3 py-1.5 border-t border-slate-900/8 flex items-center justify-between text-[10px] text-slate-600/50">
        <span className="num">
          {lastUpdate
            ? new Date(lastUpdate).toLocaleTimeString(dateLocale(locale), {
                hour: "2-digit",
                minute: "2-digit",
              })
            : ""}
        </span>
        <button
          onClick={onGoPricing}
          className="hover:text-sky-600 transition-colors font-medium"
        >
          {t("stats.priceSettings")}
        </button>
      </div>

      {/* 「＋添加服务」→ 该服务的添加凭证弹层（复用 CredentialsCard 表单，
          覆盖整个面板；添加成功后 tab 自动出现并切换，见 handleAddSubmit） */}
      {addProvider && (
        <CredentialFormDialog
          kind={CREDENTIAL_AGENT_KIND[addProvider]}
          editing={null}
          regionOptions={regionOptionsFor(addProvider, t)}
          // kimi 表单内可选凭证类型（OAuth 令牌 / API Key）
          kindSelectable={addProvider === "kimi"}
          oauth={oauthFlow}
          onOAuthSuccess={() => handleOAuthSuccess(addProvider)}
          busy={addBusy}
          error={addError}
          titleText={`${
            AGENT_VISIBILITY_OPTIONS.find((o) => o.id === addProvider)?.label ??
            addProvider
          } · ${t("credentials.add")}`}
          onCancel={() => {
            setAddProvider(null);
            setAddError(null);
          }}
          onSubmit={handleAddSubmit}
        />
      )}
    </div>
  );
}

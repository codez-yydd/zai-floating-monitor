import { useEffect, useMemo, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Currency, PricingConfig, StatsTab } from "./types";
import { fetchPin, setPin } from "./api";
import { useDataCache } from "./DataCache";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker } from "./RangePicker";
import { ZaiStatsContent } from "./ZaiStatsContent";
import { CodexPanel } from "./CodexPanel";
import { ClaudePanel } from "./ClaudePanel";
import { CursorPanel } from "./CursorPanel";
import { SummaryTab } from "./SummaryTab";
import { BrandIcon, type BrandIconName } from "./BrandIcon";
import {
  AGENT_VISIBILITY_OPTIONS,
  type AgentVisibility,
} from "./agentVisibility";
import { LanguageToggle, ThemeToggle } from "./layout";
import { useI18n } from "./i18n";
import { dateLocale } from "./i18n/locale";

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
  onGoPricing: () => void;
  onGoSync: () => void;
  onGoCompare: () => void;
  onGoReport: () => void;
  onGoSettings: () => void;
}

function loadStatsTab(agentVisibility: AgentVisibility): StatsTab {
  try {
    const saved = localStorage.getItem("zbar-tab");
    if (saved === "summary") return saved;
    if (
      saved === "zai" ||
      saved === "codex" ||
      saved === "claude" ||
      saved === "cursor"
    ) {
      return agentVisibility[saved] ? saved : "summary";
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
  onGoPricing,
  onGoSync,
  onGoCompare,
  onGoReport,
  onGoSettings,
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
  const [tab, setTab] = useState<StatsTab>(
    () => loadStatsTab(agentVisibility)
  );

  useEffect(() => {
    if (tab !== "summary" && !agentVisibility[tab]) {
      setTab("summary");
    }
  }, [agentVisibility, tab]);

  // 模式 C：「汇总」标签走词典（其余是品牌名），语言切换时随 t 重建
  const statTabs = useMemo<
    ReadonlyArray<{ id: StatsTab; label: string; brand?: BrandIconName }>
  >(
    () => [
      { id: "summary", label: t("stats.tab.summary") },
      ...AGENT_VISIBILITY_OPTIONS.map((agent) => ({
        id: agent.id,
        label: agent.label,
        brand: agent.id as BrandIconName,
      })),
    ],
    [t]
  );

  const visibleTabs = statTabs.filter((item) =>
    item.id === "summary" ? true : agentVisibility[item.id]
  );

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

  // 记忆当前标签（localStorage 异常静默，对齐 cache.ts：记忆仅锦上添花，不影响主流程）
  useEffect(() => {
    try {
      localStorage.setItem("zbar-tab", tab);
    } catch {
      /* 忽略：QuotaExceededError、隐私模式等 */
    }
  }, [tab]);

  return (
    <div className="flex flex-col h-full">
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
            <span className="text-sky-600">
              <BrandIcon brand="zai" className="h-4 w-4" />
            </span>
            <h1 className="text-[13px] font-bold text-slate-900/90 tracking-tight">
              ZCode Token
            </h1>
          </div>
          <div className="flex items-center gap-0.5">
            <button
              onClick={onGoCompare}
              className="toolbar-btn"
              title={t("stats.compare")}
            >
              📊
            </button>
            <button
              onClick={onGoReport}
              className="toolbar-btn"
              title={t("stats.report")}
            >
              📄
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
              onClick={onGoSettings}
              className="toolbar-btn"
              title={t("stats.settings")}
            >
              ⚙
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
        <div
          className={`${syncEnabled ? "mt-1.5" : "mt-2"} min-w-0 overflow-x-auto`}
          aria-label={t("stats.sourcesAria")}
        >
          <div className="flex w-max min-w-full gap-1 p-0.5 rounded-xl bg-slate-900/4">
            {visibleTabs.map((item) => {
              const t = item.id;
              const activeColors: Record<string, string> = {
                cursor: "bg-violet-500/12 text-violet-700 shadow-sm",
                claude: "bg-orange-500/12 text-orange-700 shadow-sm",
                codex: "bg-emerald-500/12 text-emerald-700 shadow-sm",
                zai: "bg-sky-500/12 text-sky-700 shadow-sm",
                summary: "bg-sky-500/15 text-sky-700 shadow-sm",
              };
              return (
                <button
                  key={t}
                  onClick={() => setTab(t)}
                  type="button"
                  aria-pressed={tab === t}
                  className={`flex shrink-0 items-center justify-center gap-1 whitespace-nowrap rounded-lg px-2.5 py-1 text-[10px] font-medium transition-all duration-150 ${
                    tab === t
                      ? activeColors[t] ?? "bg-surface text-slate-900 shadow-sm"
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
      </div>

      {/* Coding Plan 额度监控 —— 仅在 z.ai 标签显示。
          额度采样由 DataProvider 全局定时器负责，与组件挂载无关，无需 display:none hack。 */}
      {tab === "zai" && <QuotaPanel onGoSettings={onGoSettings} />}

      {/* 标签内容 */}
      {tab === "zai" ? (
        <>
          {error && (
            <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
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
        </>
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
      ) : (
        <SummaryTab
          stats={stats}
          cost={cost}
          trend={trend}
          codex={codex}
          claude={claude}
          cursor={cursor}
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
    </div>
  );
}

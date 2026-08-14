import { useEffect, useState } from "react";
import { getCurrentWindow } from "@tauri-apps/api/window";
import type { Currency, PricingConfig, StatsTab } from "./types";
import { fetchPin, setPin } from "./api";
import { useDataCache } from "./DataCache";
import { QuotaPanel } from "./QuotaPanel";
import { RangePicker } from "./RangePicker";
import { ZaiStatsContent } from "./ZaiStatsContent";
import { CursorPanel } from "./CursorPanel";
import { SummaryTab } from "./SummaryTab";

interface Props {
  currency: Currency;
  pricing: PricingConfig;
  onGoPricing: () => void;
  onGoSync: () => void;
  onGoCompare: () => void;
  onGoReport: () => void;
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
  onGoPricing,
  onGoSync,
  onGoCompare,
  onGoReport,
}: Props) {
  const cache = useDataCache();
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
    cursor,
    cursorError,
    fxRate,
    syncConfig,
    remoteDevices,
    syncEnabled,
    lastUpdate,
    refreshing,
    refresh,
  } = cache;

  // 三标签：汇总 | z.ai | Cursor
  const [tab, setTab] = useState<StatsTab>(
    () => (localStorage.getItem("zbar-tab") as StatsTab) || "summary"
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
      <div className="px-3.5 pt-3 pb-2.5 border-b border-slate-900/10">
        {/* Windows 无边框窗口拖动 */}
        <div
          className={`flex items-center justify-between mb-2.5 ${
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
          <h1 className="text-[13px] font-semibold text-slate-900/90 select-none">
            ZCode Token
          </h1>
          <div className="flex items-center gap-2.5">
            <button
              onClick={onGoCompare}
              className="text-xs text-slate-700/40 hover:text-slate-900/70 transition-colors"
              title="周额度对比"
            >
              📊
            </button>
            <button
              onClick={onGoReport}
              className="text-xs text-slate-700/40 hover:text-slate-900/70 transition-colors"
              title="用量报告"
            >
              📄
            </button>
            <button
              onClick={onGoSync}
              className={`text-xs transition-colors ${
                syncEnabled
                  ? "text-emerald-600 hover:text-emerald-700"
                  : "text-slate-700/40 hover:text-slate-900/70"
              }`}
              title={syncEnabled ? "设备同步" : "配置设备同步"}
            >
              ⇅
            </button>
            {isWindows && (
              <button
                onClick={() => {
                  const next = !pinned;
                  setPinned(next);
                  setPin(next).catch(() => setPinned(!next));
                }}
                className={`text-xs transition-colors ${
                  pinned
                    ? "text-sky-600 hover:text-sky-700"
                    : "text-slate-700/40 hover:text-slate-900/70"
                }`}
                title={pinned ? "取消常驻" : "常驻置顶"}
              >
                📌
              </button>
            )}
            <button
              onClick={refresh}
              disabled={refreshing}
              className="text-slate-700/50 hover:text-slate-900/80 text-xs transition-colors"
              title="刷新"
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
        {/* 设备筛选 + 三标签同一行，少占一排高度 */}
        <div className="mt-2 flex items-center gap-1.5">
          {syncEnabled && (
            <select
              value={deviceFilter}
              onChange={(e) => setDeviceFilter(e.target.value)}
              title="筛选设备"
              className="num w-[4.75rem] shrink-0 px-1 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60"
            >
              <option value="all">全部</option>
              <option value="local">
                本机{syncConfig?.device_name ? `（${syncConfig.device_name}）` : ""}
              </option>
              {remoteDevices
                .filter((d) => d.device_id !== syncConfig?.device_id)
                .map((d) => (
                  <option key={d.device_id} value={d.device_id}>
                    {d.device_name}（{d.device_id.slice(0, 6)}）
                  </option>
                ))}
            </select>
          )}
          <div className="flex-1 flex gap-0.5 p-0.5 rounded-lg bg-slate-900/5">
            {(["summary", "zai", "cursor"] as const).map((t) => (
              <button
                key={t}
                onClick={() => setTab(t)}
                className={`flex-1 py-1 rounded-md text-[10px] font-medium transition-colors ${
                  tab === t
                    ? t === "cursor"
                      ? "bg-white text-violet-700 shadow-sm"
                      : t === "zai"
                        ? "bg-white text-sky-700 shadow-sm"
                        : "bg-white text-slate-900 shadow-sm"
                    : "text-slate-700/50 hover:text-slate-900/70"
                }`}
              >
                {t === "summary" ? "汇总" : t === "zai" ? "Z.ai" : "Cursor"}
              </button>
            ))}
          </div>
        </div>
      </div>

      {/* Coding Plan 额度监控 —— 仅在 z.ai 标签显示。
          额度采样由 DataProvider 全局定时器负责，与组件挂载无关，无需 display:none hack。 */}
      {tab === "zai" && <QuotaPanel onGoSettings={onGoPricing} />}

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
      ) : tab === "cursor" ? (
        <CursorPanel
          snapshot={cursor}
          loading={!cursor && !cursorError}
          error={cursorError}
          currency={currency}
          fxRate={fxRate}
        />
      ) : (
        <SummaryTab
          stats={stats}
          cost={cost}
          trend={trend}
          cursor={cursor}
          currency={currency}
          bucket={trendBucket}
          fxRate={fxRate}
          pricing={pricing}
        />
      )}

      {/* 底部 */}
      <div className="px-3.5 py-2 border-t border-slate-900/10 flex items-center justify-between text-[10px] text-slate-700/50">
        <span className="num">
          {lastUpdate
            ? new Date(lastUpdate).toLocaleTimeString("zh-CN", {
                hour: "2-digit",
                minute: "2-digit",
              })
            : ""}
        </span>
        <button
          onClick={onGoPricing}
          className="hover:text-sky-600 transition-colors"
        >
          ⚙ 价格设置
        </button>
      </div>
    </div>
  );
}

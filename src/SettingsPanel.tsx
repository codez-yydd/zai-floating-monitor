import { useEffect, useRef, useState } from "react";
import type { CursorConfig, ShortcutConfig } from "./types";
import {
  fetchFxRate,
  getCursorConfig,
  getShortcutConfig,
  setCursorConfig,
  setShortcutConfig,
} from "./api";
import {
  disable as disableAutostart,
  enable as enableAutostart,
  isEnabled as isAutostartEnabled,
} from "@tauri-apps/plugin-autostart";
import {
  applyPanelAlpha,
  loadPanelAlpha,
  persistPanelAlpha,
} from "./appearance";
import { BrandIcon } from "./BrandIcon";
import { AccountsCard } from "./AccountsCard";
import {
  PageShell,
  PageHeader,
  PageBody,
  SettingsCard,
  PillGroup,
  PillButton,
  BtnPrimary,
  AlertBanner,
} from "./layout";
import {
  AGENT_VISIBILITY_OPTIONS,
  type AgentId,
  type AgentVisibility,
} from "./agentVisibility";
import { useI18n } from "./i18n";

interface Props {
  onBack: () => void;
  agentVisibility: AgentVisibility;
  onAgentVisibilityChange: (id: AgentId, visible: boolean) => void;
}

/// 汇率最近获取时间的显示格式：MM-DD HH:mm（本地时区）
function fmtFxTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/**
 * 设置页：面板透明度 + 从价格设置页搬来的非价格配置
 * （开机自启 / 汇率 / 全局快捷键）。
 * 单列滚动，改完即存（无整页保存按钮）。
 */
export function SettingsPanel({
  onBack,
  agentVisibility,
  onAgentVisibilityChange,
}: Props) {
  const { locale, t, setLocale } = useI18n();
  const [error, setError] = useState<string | null>(null);
  // 配置加载成功后才允许保存/更新/应用：加载失败时组件停在默认值，
  // 若仍可保存会把默认汇率等值写回后端覆盖真实配置
  const [loaded, setLoaded] = useState(false);

  // ===== 外观：面板透明度（localStorage 持久化，改完即时生效）=====
  const [alpha, setAlpha] = useState<number>(() => loadPanelAlpha());
  // ===== 开机自启 =====
  const [autostartEnabled, setAutostartEnabled] = useState(false);
  const [autostartLoaded, setAutostartLoaded] = useState(false);
  const [savingAutostart, setSavingAutostart] = useState(false);
  const [autostartError, setAutostartError] = useState<string | null>(null);
  // 透明度持久化防抖：拖动滑块时只实时应用 DOM，写盘按 300ms 节流
  const alphaPersistTimer = useRef<number | undefined>(undefined);
  // 最新透明度镜像：卸载清理的闭包读不到最新 state，从 ref 取
  const lastAlphaRef = useRef(alpha);

  // ===== 全局快捷键配置 =====
  const [shortcutCfg, setShortcutCfg] = useState<ShortcutConfig | null>(null);
  const [shortcutDraft, setShortcutDraft] = useState("");
  const [savingShortcut, setSavingShortcut] = useState(false);
  const [shortcutSavedFlash, setShortcutSavedFlash] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);

  // ===== Cursor 配置（设置页仅消费其中的汇率字段；认证自动读 Cursor 应用）=====
  const [cursorCfg, setCursorCfg] = useState<CursorConfig>({
    cookie_source: "auto",
    cookie_header: "",
    usd_cny_rate: 7.2,
    fx_rate_auto: true,
    fx_rate_fetched_at: null,
    fx_rate_source: null,
  });
  const [savingCursor, setSavingCursor] = useState(false);
  const [cursorSavedFlash, setCursorSavedFlash] = useState(false);
  // 汇率「立即更新」进行中（防重复点击）
  const [fxUpdating, setFxUpdating] = useState(false);
  // 最近一次「立即更新」的结果反馈（✓ 成功 / ✗ 失败）
  const [fxUpdateResult, setFxUpdateResult] = useState<string | null>(null);

  // 汇率手动输入草稿：失焦再解析，避免清空输入时被 parseFloat(NaN) 立即跳回默认值
  const [fxDraft, setFxDraft] = useState<string | null>(null);

  // 卸载时冲掉未落盘的透明度防抖（离开设置页前保证最后一次调整已持久化）
  useEffect(() => {
    return () => {
      if (alphaPersistTimer.current !== undefined) {
        window.clearTimeout(alphaPersistTimer.current);
        persistPanelAlpha(lastAlphaRef.current);
      }
    };
  }, []);

  useEffect(() => {
    Promise.all([getShortcutConfig(), getCursorConfig()])
      .then(([s, cc]) => {
        setShortcutCfg(s);
        setShortcutDraft(s.accelerator);
        setCursorCfg(cc);
        setLoaded(true);
      })
      .catch((e) => setError(String(e)));
  }, []);

  useEffect(() => {
    isAutostartEnabled()
      .then(setAutostartEnabled)
      .catch((e) =>
        setAutostartError(t("settings.autostartReadFail", { msg: String(e) }))
      )
      .finally(() => setAutostartLoaded(true));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 保存 Cursor 配置（设置页改动落盘：手动汇率输入、自动更新开关）
  const handleSaveCursor = async () => {
    setSavingCursor(true);
    setError(null);
    try {
      await setCursorConfig(cursorCfg);
      setCursorSavedFlash(true);
      setTimeout(() => setCursorSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingCursor(false);
    }
  };

  // 立即联网更新汇率。nextCfg 用于勾选自动时（state 尚未生效）传入最新配置，
  // 保证后端 load→改→save 基于最新值，不覆盖未落盘的改动。
  const handleFetchFxRate = async (nextCfg?: CursorConfig) => {
    setFxUpdating(true);
    setFxUpdateResult(null);
    try {
      // 先保存当前配置再联网（后端在磁盘配置上合并汇率，与测试连接同款顺序）
      await setCursorConfig(nextCfg ?? cursorCfg);
      const [rate, source] = await fetchFxRate();
      setCursorCfg((c) => ({
        ...c,
        usd_cny_rate: rate,
        fx_rate_fetched_at: Date.now(),
        fx_rate_source: source,
      }));
      setFxUpdateResult(
        `✓ ${rate.toFixed(2)}${locale === "zh" ? "（" : " ("}${source}${locale === "zh" ? "）" : ")"}`
      );
    } catch (e) {
      // 失败保留旧汇率值，只提示错误
      setFxUpdateResult(`✗ ${String(e)}`);
    } finally {
      setFxUpdating(false);
    }
  };

  // 保存并应用快捷键（注册失败时提示用户改键）
  const handleSaveShortcut = async () => {
    if (!shortcutCfg) return;
    setSavingShortcut(true);
    setShortcutError(null);
    try {
      const next = { ...shortcutCfg, accelerator: shortcutDraft.trim() };
      await setShortcutConfig(next);
      setShortcutCfg(next);
      setShortcutSavedFlash(true);
      setTimeout(() => setShortcutSavedFlash(false), 1500);
    } catch (e) {
      setShortcutError(String(e));
    } finally {
      setSavingShortcut(false);
    }
  };

  // 应用开机自启设置。插件会在 Windows 写入当前用户启动项，在 macOS
  // 写入当前用户的 LaunchAgent，不需要管理员权限。
  const handleAutostartChange = async (enabled: boolean) => {
    setSavingAutostart(true);
    setAutostartError(null);
    try {
      if (enabled) {
        await enableAutostart();
      } else {
        await disableAutostart();
      }
      setAutostartEnabled(enabled);
    } catch (e) {
      setAutostartError(
        enabled
          ? t("settings.autostartFailOn", { msg: String(e) })
          : t("settings.autostartFailOff", { msg: String(e) })
      );
    } finally {
      setSavingAutostart(false);
    }
  };

  return (
    <PageShell>
      <PageHeader title={t("settings.title")} onBack={onBack} />

      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}

        <SettingsCard title={t("settings.panelOpacity")}>
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-slate-700/55 shrink-0">
              {t("settings.opacity")}
            </span>
            <input
              type="range"
              min="0.2"
              max="1"
              step="0.05"
              value={alpha}
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                setAlpha(v);
                applyPanelAlpha(v);
                // 实时应用 + 防抖写盘：一次拖动只落一次盘
                lastAlphaRef.current = v;
                if (alphaPersistTimer.current !== undefined) {
                  window.clearTimeout(alphaPersistTimer.current);
                }
                alphaPersistTimer.current = window.setTimeout(() => {
                  persistPanelAlpha(lastAlphaRef.current);
                  alphaPersistTimer.current = undefined;
                }, 300);
              }}
              className="accent-sky-500 flex-1"
            />
            <span className="num text-[10px] text-slate-700/65 w-8 text-right shrink-0">
              {Math.round(alpha * 100)}%
            </span>
          </div>
          <p className="text-[9px] text-slate-500 mt-1.5 leading-relaxed">
            {t("settings.opacityHint")}
          </p>
        </SettingsCard>

        {/* 语言：与顶栏 LanguageToggle 读写同一 Context，切换后全站即时同步 */}
        <SettingsCard title={t("settings.language")}>
          <PillGroup>
            <PillButton
              active={locale === "zh"}
              onClick={() => setLocale("zh")}
            >
              {t("settings.langZh")}
            </PillButton>
            <PillButton
              active={locale === "en"}
              onClick={() => setLocale("en")}
            >
              {t("settings.langEn")}
            </PillButton>
          </PillGroup>
        </SettingsCard>

        <SettingsCard
          title={t("settings.autostart")}
          action={
            <label className="flex items-center gap-1 text-[10px] text-slate-600 cursor-pointer">
              <input
                type="checkbox"
                checked={autostartEnabled}
                onChange={(e) => handleAutostartChange(e.target.checked)}
                disabled={!autostartLoaded || savingAutostart}
                className="accent-sky-500 h-3 w-3 disabled:opacity-40"
              />
              {savingAutostart ? t("settings.applying") : t("settings.enable")}
            </label>
          }
        >
          <p className="text-[9px] text-slate-500 leading-relaxed">
            {t("settings.autostartHint")}
          </p>
          {!autostartLoaded && (
            <p className="text-[9px] text-slate-500 mt-1">{t("settings.readingState")}</p>
          )}
          {autostartError && (
            <p className="text-[9px] text-rose-600 mt-1 leading-relaxed break-all">
              {autostartError}
            </p>
          )}
        </SettingsCard>

        <SettingsCard title={t("settings.sources")} action={<span className="text-[9px] text-slate-500">{t("settings.instant")}</span>} hint={t("settings.sourcesHint")}>
            {AGENT_VISIBILITY_OPTIONS.map((agent) => (
              <label
                key={agent.id}
                className="flex items-center justify-between gap-2 rounded-md px-1.5 py-1 hover:bg-slate-900/5 cursor-pointer transition-colors"
              >
                <span className="flex items-center gap-1.5 min-w-0">
                  <BrandIcon
                    brand={agent.id}
                    className="h-3.5 w-3.5 shrink-0 text-slate-700/65"
                  />
                  <span className="min-w-0">
                    <span className="block text-[10px] text-slate-900/80">
                      {agent.label}
                    </span>
                    <span className="block text-[9px] text-slate-700/45 truncate">
                      {t(agent.descriptionKey)}
                    </span>
                  </span>
                </span>
                <input
                  type="checkbox"
                  checked={agentVisibility[agent.id]}
                  onChange={(e) =>
                    onAgentVisibilityChange(agent.id, e.target.checked)
                  }
                  className="accent-sky-500 h-3 w-3 shrink-0"
                />
              </label>
            ))}
        </SettingsCard>

        {/* ZCode 账号切换：捕获/切换登录快照（数据自管，不进 DataCache） */}
        <AccountsCard />

        {/* 汇率：USD→CNY 折算（价格只存美元，人民币花费实时折算） */}
        <SettingsCard
          title={t("settings.fxCard")}
          action={
            <BtnPrimary onClick={handleSaveCursor} disabled={savingCursor || !loaded}>
              {savingCursor ? t("common.saving") : cursorSavedFlash ? t("common.saved") : t("common.save")}
            </BtnPrimary>
          }
        >
          <div className="flex items-center gap-2">
            <span className="text-[10px] text-slate-700/60 w-12 shrink-0">
              USD→CNY
            </span>
            <input
              type="number"
              step="0.1"
              min="0.1"
              value={
                fxDraft !== null ? fxDraft : String(cursorCfg.usd_cny_rate)
              }
              readOnly={cursorCfg.fx_rate_auto}
              title={
                cursorCfg.fx_rate_auto
                  ? t("settings.fxAutoNote")
                  : undefined
              }
              onChange={(e) => setFxDraft(e.target.value)}
              onBlur={() => {
                // 失焦才解析：清空/非法输入保持原值，不被立即跳回默认汇率
                if (fxDraft !== null) {
                  const v = parseFloat(fxDraft);
                  if (v > 0 && v !== cursorCfg.usd_cny_rate) {
                    setCursorCfg({ ...cursorCfg, usd_cny_rate: v });
                  }
                  setFxDraft(null);
                }
              }}
              className={`num w-20 px-2 py-0.5 rounded-md bg-surface/60 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60 ${
                cursorCfg.fx_rate_auto
                  ? "opacity-60 cursor-not-allowed"
                  : ""
              }`}
            />
            <span className="text-[9px] text-slate-700/45 truncate">
              {cursorCfg.fx_rate_fetched_at
                ? `${cursorCfg.fx_rate_source ?? t("settings.fxUnknownSource")} · ${fmtFxTime(cursorCfg.fx_rate_fetched_at)}`
                : t("settings.fxNever")}
            </span>
          </div>
          <div className="flex items-center gap-2 mt-1">
            <label
              className="flex items-center gap-1 text-[9px] text-slate-700/55 cursor-pointer"
              title={t("settings.fxDailyTitle")}
            >
              <input
                type="checkbox"
                checked={cursorCfg.fx_rate_auto}
                onChange={(e) => {
                  const next = {
                    ...cursorCfg,
                    fx_rate_auto: e.target.checked,
                  };
                  setCursorCfg(next);
                  // 勾选自动且从未获取过：顺带立即拉一次，避免长期显示"尚未联网获取"
                  if (e.target.checked && !cursorCfg.fx_rate_fetched_at) {
                    handleFetchFxRate(next);
                  }
                }}
                className="accent-sky-500 w-3 h-3"
              />
              {t("settings.fxDaily")}
            </label>
            <button
              onClick={() => handleFetchFxRate()}
              disabled={fxUpdating || !loaded}
              title={t("settings.updateNowTitle")}
              className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {fxUpdating ? t("settings.updating") : t("settings.updateNow")}
            </button>
            {fxUpdateResult && (
              <span
                className={`text-[9px] truncate ${
                  fxUpdateResult.startsWith("✓")
                    ? "text-emerald-600"
                    : "text-rose-600"
                }`}
              >
                {fxUpdateResult}
              </span>
            )}
          </div>
          {cursorCfg.fx_rate_auto && (
            <p className="text-[8px] text-slate-700/40 mt-0.5">
              {t("settings.fxAutoNote")}
            </p>
          )}
          <p className="text-[8px] text-slate-700/40 mt-0.5">
            {t("settings.fxNote")}
          </p>
        </SettingsCard>

        {shortcutCfg && (
          <SettingsCard
            title={t("settings.shortcut")}
            action={
              <div className="flex items-center gap-2">
                <label className="flex items-center gap-1 text-[10px] text-slate-600 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={shortcutCfg.enabled}
                    onChange={(e) =>
                      setShortcutCfg({ ...shortcutCfg, enabled: e.target.checked })
                    }
                    className="accent-sky-500 w-3 h-3"
                  />
                  {t("settings.enable")}
                </label>
                <BtnPrimary onClick={handleSaveShortcut} disabled={savingShortcut || !loaded}>
                  {savingShortcut ? t("settings.applying") : shortcutSavedFlash ? t("settings.applied") : t("settings.apply")}
                </BtnPrimary>
              </div>
            }
            hint={t("settings.shortcutHint")}
          >
            <div className="input-group">
              <input
                type="text"
                value={shortcutDraft}
                placeholder="alt+shift+z"
                onChange={(e) => setShortcutDraft(e.target.value)}
                className="num"
              />
            </div>
            {shortcutError && (
              <p className="text-[10px] text-rose-600 mt-1.5 leading-relaxed">{shortcutError}</p>
            )}
          </SettingsCard>
        )}
      </PageBody>
    </PageShell>
  );
}

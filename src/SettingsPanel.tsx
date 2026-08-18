import { useEffect, useRef, useState } from "react";
import type {
  CursorConfig,
  QuotaConfig,
  QuotaEndpoint,
  ShortcutConfig,
} from "./types";
import {
  cursorDebug,
  fetchFxRate,
  fetchQuotaConfig,
  getCursorConfig,
  getShortcutConfig,
  saveQuotaConfig,
  setCursorConfig,
  setShortcutConfig,
  testCursorAuth,
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
import {
  PageShell,
  PageHeader,
  PageBody,
  SettingsCard,
  PillGroup,
  PillButton,
  BtnPrimary,
  BtnSecondary,
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
 * （开机自启 / Coding Plan 额度监控 / Cursor 统计 / 全局快捷键）。
 * 单列滚动，改完即存（无整页保存按钮）。
 */
export function SettingsPanel({
  onBack,
  agentVisibility,
  onAgentVisibilityChange,
}: Props) {
  const { locale, t, setLocale } = useI18n();
  const [error, setError] = useState<string | null>(null);
  // 配置加载成功后才允许保存/测试/应用：加载失败时组件停在默认值，
  // 若仍可保存会把空 token 等默认值写回后端覆盖真实配置
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

  // ===== Coding Plan 额度查询配置 =====
  const [quotaCfg, setQuotaCfg] = useState<QuotaConfig>({
    token: "",
    endpoint: "cn",
  });
  const [tokenDraft, setTokenDraft] = useState<string>("");
  const [showToken, setShowToken] = useState(false);
  const [savingQuota, setSavingQuota] = useState(false);
  const [quotaSavedFlash, setQuotaSavedFlash] = useState(false);

  // ===== 全局快捷键配置 =====
  const [shortcutCfg, setShortcutCfg] = useState<ShortcutConfig | null>(null);
  const [shortcutDraft, setShortcutDraft] = useState("");
  const [savingShortcut, setSavingShortcut] = useState(false);
  const [shortcutSavedFlash, setShortcutSavedFlash] = useState(false);
  const [shortcutError, setShortcutError] = useState<string | null>(null);

  // ===== Cursor 配置 =====
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
  const [cursorTesting, setCursorTesting] = useState(false);
  const [cursorTestResult, setCursorTestResult] = useState<string | null>(null);
  const [cursorDebugInfo, setCursorDebugInfo] = useState<string | null>(null);
  const [cursorDebugging, setCursorDebugging] = useState(false);
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
    Promise.all([fetchQuotaConfig(), getShortcutConfig(), getCursorConfig()])
      .then(([q, s, cc]) => {
        setQuotaCfg(q);
        setTokenDraft(q.token);
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

  const handleSaveQuota = async () => {
    const merged: QuotaConfig = {
      ...quotaCfg,
      token: tokenDraft.trim(),
    };
    setSavingQuota(true);
    setError(null);
    try {
      await saveQuotaConfig(merged);
      setQuotaCfg(merged);
      setQuotaSavedFlash(true);
      setTimeout(() => setQuotaSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingQuota(false);
    }
  };

  // 保存 Cursor 配置
  const handleSaveCursor = async () => {
    setSavingCursor(true);
    setError(null);
    setCursorTestResult(null);
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

  // 测试 Cursor 认证
  const handleTestCursor = async () => {
    setCursorTesting(true);
    setCursorTestResult(null);
    try {
      // 先保存当前配置，再用最新配置测试
      await setCursorConfig(cursorCfg);
      const [email, name, membership] = await testCursorAuth();
      if (email) {
        setCursorTestResult(
          membership
            ? t("settings.connectedEmailPlan", { email, plan: membership })
            : t("settings.connectedEmail", { email })
        );
      } else if (name) {
        setCursorTestResult(t("settings.connectedName", { name }));
      } else {
        setCursorTestResult(t("settings.authOk"));
      }
    } catch (e) {
      setCursorTestResult(`✗ ${String(e)}`);
    } finally {
      setCursorTesting(false);
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

        <SettingsCard
          title={t("quota.title")}
          action={
            <BtnPrimary onClick={handleSaveQuota} disabled={savingQuota || !loaded}>
              {savingQuota ? t("common.saving") : quotaSavedFlash ? t("common.saved") : t("common.save")}
            </BtnPrimary>
          }
        >
          {/* Token 输入 */}
          <label className="flex flex-col gap-0.5 text-[10px]">
            <span className="text-slate-700/55">API Token</span>
            <div className="flex items-center rounded-md bg-surface/60 border border-slate-900/10 focus-within:border-sky-400/60 focus-within:ring-1 focus-within:ring-sky-400/40 transition-colors">
              <input
                type={showToken ? "text" : "password"}
                value={tokenDraft}
                placeholder={t("settings.tokenPh")}
                onChange={(e) => setTokenDraft(e.target.value)}
                className="num w-full px-1.5 py-1 text-left bg-transparent text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none text-[11px]"
              />
              <button
                onClick={() => setShowToken((v) => !v)}
                className="px-1.5 text-slate-700/40 hover:text-slate-900/70 transition-colors text-[10px] shrink-0"
                title={showToken ? t("common.hide") : t("common.show")}
              >
                {showToken ? "🙈" : "👁"}
              </button>
            </div>
          </label>
          {/* 端点切换 */}
          <div className="flex items-center justify-between mt-2">
            <span className="text-[10px] text-slate-700/55">{t("settings.endpoint")}</span>
            <div className="flex gap-1">
              {(["cn", "global"] as QuotaEndpoint[]).map((ep) => (
                <button
                  key={ep}
                  onClick={() =>
                    setQuotaCfg((c) => ({ ...c, endpoint: ep }))
                  }
                  className={`px-2 py-0.5 rounded-md text-[10px] transition-colors ${
                    quotaCfg.endpoint === ep
                      ? "bg-sky-500 text-white"
                      : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10"
                  }`}
                >
                  {ep === "cn" ? t("settings.endpointCn") : t("settings.endpointGlobal")}
                </button>
              ))}
            </div>
          </div>
          <p className="text-[9px] text-slate-500 mt-1.5 leading-relaxed">
            {t("settings.endpointHint")}
          </p>
        </SettingsCard>

        <SettingsCard
          title={t("settings.cursorStats")}
          action={
            <div className="flex items-center gap-1.5">
              <BtnSecondary onClick={async () => {
                setCursorDebugging(true);
                setCursorDebugInfo(null);
                try {
                  const info = await cursorDebug();
                  setCursorDebugInfo(
                    `${t("settings.debugSource")}: ${info.cookie_source}\nDB: ${info.db_found ? t("settings.debugDbFound") : t("settings.debugDbMissing")}\nUserID: ${info.user_id}\nEvents HTTP: ${info.events_status}\n${t("settings.debugResponse")}: ${info.events_body_excerpt}`
                  );
                } catch (e) {
                  setCursorDebugInfo(t("settings.debugFailed", { msg: String(e) }));
                } finally {
                  setCursorDebugging(false);
                }
              }} disabled={cursorDebugging || !loaded}>
                {cursorDebugging ? t("settings.debugging") : t("settings.debug")}
              </BtnSecondary>
              <BtnSecondary onClick={handleTestCursor} disabled={cursorTesting || !loaded}>
                {cursorTesting ? t("settings.testing") : t("settings.test")}
              </BtnSecondary>
              <BtnPrimary onClick={handleSaveCursor} disabled={savingCursor || !loaded}>
                {savingCursor ? t("common.saving") : cursorSavedFlash ? t("common.saved") : t("common.save")}
              </BtnPrimary>
            </div>
          }
        >

          {/* Cookie 来源切换 */}
          <div className="flex items-center gap-2 mb-1.5">
            <span className="text-[10px] text-slate-700/60 w-12 shrink-0">
              {t("settings.auth")}
            </span>
            <div className="flex gap-1">
              {(["auto", "manual"] as const).map((src) => (
                <button
                  key={src}
                  onClick={() =>
                    setCursorCfg({ ...cursorCfg, cookie_source: src })
                  }
                  className={`px-2 py-0.5 rounded text-[10px] transition-colors ${
                    cursorCfg.cookie_source === src
                      ? "bg-sky-500/20 text-sky-700"
                      : "text-slate-700/45 hover:text-slate-900/70"
                  }`}
                >
                  {src === "auto" ? t("settings.authAuto") : t("settings.authManual")}
                </button>
              ))}
            </div>
          </div>

          {/* 手动 Cookie 输入 */}
          {cursorCfg.cookie_source === "manual" && (
            <input
              type="text"
              value={cursorCfg.cookie_header}
              onChange={(e) =>
                setCursorCfg({
                  ...cursorCfg,
                  cookie_header: e.target.value,
                })
              }
              placeholder={t("settings.cookiePh")}
              className="w-full px-2 py-1 rounded-md bg-surface/60 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60 mb-1.5"
            />
          )}

          {/* 汇率：自动获取（每日后台刷一次）+ 手动立即更新 + 保留手动输入 */}
          <div className="mb-1">
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
          </div>

          {cursorTestResult && (
            <p
              className={`text-[9px] mt-1 leading-relaxed ${
                cursorTestResult.startsWith("✓")
                  ? "text-emerald-600"
                  : "text-rose-600"
              }`}
            >
              {cursorTestResult}
            </p>
          )}
          {cursorCfg.cookie_source === "auto" && (
            <p className="text-[9px] text-slate-700/45 mt-1 leading-relaxed">
              {t("settings.cursorAutoHint")}
            </p>
          )}
          {cursorDebugInfo && (
            <pre className="text-[8px] text-slate-600 mt-1.5 p-1.5 rounded-lg bg-slate-900/5 overflow-x-auto whitespace-pre-wrap break-all max-h-32 overflow-y-auto font-mono">
              {cursorDebugInfo}
            </pre>
          )}
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

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
  applyPanelAlpha,
  applyTheme,
  loadPanelAlpha,
  loadTheme,
  persistPanelAlpha,
  persistTheme,
} from "./appearance";
import type { Theme } from "./appearance";

interface Props {
  onBack: () => void;
}

/// 汇率最近获取时间的显示格式：MM-DD HH:mm（本地时区）
function fmtFxTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/**
 * 设置页：外观（主题/面板透明度）+ 从价格设置页搬来的非价格配置
 * （Coding Plan 额度监控 / Cursor 统计 / 全局快捷键）。
 * 单列滚动，改完即存（无整页保存按钮）。
 */
export function SettingsPanel({ onBack }: Props) {
  const [error, setError] = useState<string | null>(null);
  // 配置加载成功后才允许保存/测试/应用：加载失败时组件停在默认值，
  // 若仍可保存会把空 token 等默认值写回后端覆盖真实配置
  const [loaded, setLoaded] = useState(false);

  // ===== 外观：主题与面板透明度（localStorage 持久化，改完即时生效）=====
  const [theme, setTheme] = useState<Theme>(() => loadTheme());
  const [alpha, setAlpha] = useState<number>(() => loadPanelAlpha());
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
          `✓ 已连接：${email}${membership ? `（${membership}）` : ""}`
        );
      } else if (name) {
        setCursorTestResult(`✓ 已连接：${name}`);
      } else {
        setCursorTestResult("✓ 认证成功");
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
      setFxUpdateResult(`✓ ${rate.toFixed(2)}（${source}）`);
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

  return (
    // 整页单一滚动，与价格设置页同款骨架
    <div className="h-full overflow-y-auto">
      {/* 顶部 */}
      <div className="px-3.5 py-2.5 border-b border-slate-900/10">
        <div className="flex items-center justify-between">
          <button
            onClick={onBack}
            className="text-xs text-slate-700/60 hover:text-sky-600 transition-colors"
          >
            ← 返回
          </button>
          <h1 className="text-[13px] font-semibold text-slate-900/90">设置</h1>
          {/* 占位撑开右侧，保持标题居中 */}
          <span className="text-xs w-8" />
        </div>
      </div>

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      <div className="px-3.5 py-2.5 space-y-2">
        {/* ===== 外观 ===== */}
        <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-[11px] font-medium text-slate-900/85">
              外观
            </span>
          </div>
          {/* 主题切换 */}
          <div className="flex items-center justify-between">
            <span className="text-[10px] text-slate-700/55">主题</span>
            <div className="flex gap-1">
              {(["light", "dark"] as Theme[]).map((t) => (
                <button
                  key={t}
                  onClick={() => {
                    setTheme(t);
                    applyTheme(t);
                    persistTheme(t);
                  }}
                  className={`px-2 py-0.5 rounded-md text-[10px] transition-colors ${
                    theme === t
                      ? "bg-sky-500 text-white"
                      : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10 hover:text-slate-900/80"
                  }`}
                >
                  {t === "light" ? "☀ 亮色" : "🌙 暗色"}
                </button>
              ))}
            </div>
          </div>
          {/* 面板透明度 */}
          <div className="flex items-center gap-2 mt-2">
            <span className="text-[10px] text-slate-700/55 shrink-0">
              透明度
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
          <p className="text-[9px] text-slate-700/45 mt-1.5 leading-relaxed">
            调整面板背景透明度，值越低毛玻璃越透；暗色主题建议保持 60%
            以上，过低时文字可能不清晰
          </p>
        </div>

        {/* ===== Coding Plan 额度查询配置 ===== */}
        <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-[11px] font-medium text-slate-900/85">
              Coding Plan 额度监控
            </span>
            <button
              onClick={handleSaveQuota}
              disabled={savingQuota || !loaded}
              className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
            >
              {savingQuota
                ? "保存中…"
                : quotaSavedFlash
                  ? "已保存 ✓"
                  : "保存"}
            </button>
          </div>
          {/* Token 输入 */}
          <label className="flex flex-col gap-0.5 text-[10px]">
            <span className="text-slate-700/55">API Token</span>
            <div className="flex items-center rounded-md bg-surface/60 border border-slate-900/10 focus-within:border-sky-400/60 focus-within:ring-1 focus-within:ring-sky-400/40 transition-colors">
              <input
                type={showToken ? "text" : "password"}
                value={tokenDraft}
                placeholder="粘贴 Coding Plan API Token"
                onChange={(e) => setTokenDraft(e.target.value)}
                className="num w-full px-1.5 py-1 text-left bg-transparent text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none text-[11px]"
              />
              <button
                onClick={() => setShowToken((v) => !v)}
                className="px-1.5 text-slate-700/40 hover:text-slate-900/70 transition-colors text-[10px] shrink-0"
                title={showToken ? "隐藏" : "显示"}
              >
                {showToken ? "🙈" : "👁"}
              </button>
            </div>
          </label>
          {/* 端点切换 */}
          <div className="flex items-center justify-between mt-2">
            <span className="text-[10px] text-slate-700/55">端点</span>
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
                  {ep === "cn" ? "🇨🇳 国内" : "🌐 国际"}
                </button>
              ))}
            </div>
          </div>
          <p className="text-[9px] text-slate-700/45 mt-1.5 leading-relaxed">
            Token 从智谱开放平台获取。国内用户选「国内」端点。
          </p>
        </div>

        {/* ===== Cursor 用量配置 ===== */}
        <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-[11px] font-medium text-slate-900/85">
              Cursor 统计
            </span>
            <div className="flex items-center gap-2">
              <button
                onClick={async () => {
                  setCursorDebugging(true);
                  setCursorDebugInfo(null);
                  try {
                    const info = await cursorDebug();
                    setCursorDebugInfo(
                      `来源: ${info.cookie_source}\nDB: ${info.db_found ? "已找到" : "未找到"}\nUserID: ${info.user_id}\nEvents HTTP: ${info.events_status}\n响应: ${info.events_body_excerpt}`
                    );
                  } catch (e) {
                    setCursorDebugInfo(`诊断失败: ${e}`);
                  } finally {
                    setCursorDebugging(false);
                  }
                }}
                disabled={cursorDebugging || !loaded}
                className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {cursorDebugging ? "诊断中…" : "诊断"}
              </button>
              <button
                onClick={handleTestCursor}
                disabled={cursorTesting || !loaded}
                className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 disabled:opacity-40 disabled:cursor-not-allowed transition-colors"
              >
                {cursorTesting ? "测试中…" : "测试连接"}
              </button>
              <button
                onClick={handleSaveCursor}
                disabled={savingCursor || !loaded}
                className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
              >
                {savingCursor
                  ? "保存中…"
                  : cursorSavedFlash
                    ? "已保存 ✓"
                    : "保存"}
              </button>
            </div>
          </div>

          {/* Cookie 来源切换 */}
          <div className="flex items-center gap-2 mb-1.5">
            <span className="text-[10px] text-slate-700/60 w-12 shrink-0">
              认证
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
                  {src === "auto" ? "自动（读 Cursor 应用）" : "手动 Cookie"}
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
              placeholder="粘贴 cursor.com 请求的 Cookie 头"
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
                    ? "自动更新中，取消勾选可手动输入"
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
                  ? `${cursorCfg.fx_rate_source ?? "未知来源"} · ${fmtFxTime(cursorCfg.fx_rate_fetched_at)}`
                  : "尚未联网获取"}
              </span>
            </div>
            <div className="flex items-center gap-2 mt-1">
              <label
                className="flex items-center gap-1 text-[9px] text-slate-700/55 cursor-pointer"
                title="后台每天自动联网刷新一次汇率"
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
                每日自动更新汇率
              </label>
              <button
                onClick={() => handleFetchFxRate()}
                disabled={fxUpdating || !loaded}
                title="立即联网获取最新汇率（多个免费数据源自动容错）"
                className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
              >
                {fxUpdating ? "更新中…" : "立即更新"}
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
                自动更新中，取消勾选可手动输入
              </p>
            )}
            <p className="text-[8px] text-slate-700/40 mt-0.5">
              模型价格只存美元，人民币花费按此汇率自动折算（价格设置页的 ¥ 视图同源）。
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
              自动读取 Cursor 应用的本地登录凭据。请确保 Cursor 已安装并登录。
            </p>
          )}
          {cursorDebugInfo && (
            <pre className="text-[8px] text-slate-700/60 mt-1.5 p-1.5 rounded bg-slate-900/5 overflow-x-auto whitespace-pre-wrap break-all max-h-32 overflow-y-auto font-mono">
              {cursorDebugInfo}
            </pre>
          )}
        </div>

        {/* ===== 全局快捷键 ===== */}
        {shortcutCfg && (
          <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
            <div className="flex items-center justify-between mb-1.5">
              <span className="text-[11px] font-medium text-slate-900/85">
                ⌨ 全局快捷键
              </span>
              <div className="flex items-center gap-2">
                <label className="flex items-center gap-1 text-[10px] text-slate-700/60 cursor-pointer">
                  <input
                    type="checkbox"
                    checked={shortcutCfg.enabled}
                    onChange={(e) =>
                      setShortcutCfg({
                        ...shortcutCfg,
                        enabled: e.target.checked,
                      })
                    }
                    className="accent-sky-500 w-3 h-3"
                  />
                  启用
                </label>
                <button
                  onClick={handleSaveShortcut}
                  disabled={savingShortcut || !loaded}
                  className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
                >
                  {savingShortcut
                    ? "应用中…"
                    : shortcutSavedFlash
                      ? "已应用 ✓"
                      : "应用"}
                </button>
              </div>
            </div>
            <p className="text-[9px] text-slate-700/45 mb-2 leading-relaxed">
              唤起/隐藏面板。格式如 alt+shift+z（修饰键用 ctrl/alt/shift/cmd，主键用字母/数字）。
            </p>
            <div className="flex items-center rounded-md bg-surface/60 border border-slate-900/10 focus-within:border-sky-400/60 focus-within:ring-1 focus-within:ring-sky-400/40 transition-colors">
              <input
                type="text"
                value={shortcutDraft}
                placeholder="alt+shift+z"
                onChange={(e) => setShortcutDraft(e.target.value)}
                className="num w-full px-2 py-1 text-left bg-transparent text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none text-[11px]"
              />
            </div>
            {shortcutError && (
              <p className="text-[10px] text-rose-600 mt-1.5 leading-relaxed">
                {shortcutError}
              </p>
            )}
          </div>
        )}
      </div>
    </div>
  );
}

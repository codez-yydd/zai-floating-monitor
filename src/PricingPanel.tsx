import { useEffect, useState } from "react";
import type {
  ApplyPriceItem,
  Currency,
  CursorConfig,
  ModelInfo,
  ModelPrice,
  PeakConfig,
  PeakSegment,
  PlanType,
  PricingConfig,
  PricingDiff,
  QuotaConfig,
  QuotaEndpoint,
  ShortcutConfig,
} from "./types";
import { MASK_WEEKDAY } from "./types";
import {
  applyPricingUpdates,
  checkPricingUpdates,
  fetchModels,
  fetchPricing,
  fetchQuotaConfig,
  getCursorConfig,
  getPeakConfig,
  getShortcutConfig,
  savePricing,
  saveQuotaConfig,
  setCursorConfig,
  setPeakConfig,
  setPlanType,
  setShortcutConfig,
  testCursorAuth,
  cursorDebug,
} from "./api";

interface Props {
  currency: Currency;
  onCurrencyChange: (c: Currency) => void;
  onBack: () => void;
}

const FIELDS: { key: keyof ModelPrice; label: string }[] = [
  { key: "input", label: "输入" },
  { key: "output", label: "输出" },
  { key: "cache_read", label: "缓存" },
];

export function PricingPanel({ currency, onCurrencyChange, onBack }: Props) {
  const [pricing, setPricing] = useState<PricingConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 草稿：把每个输入框的编辑值作为字符串暂存，避免小数点输入被 parseFloat 吞掉。
  // key = `${modelId}|${field}`
  const [draft, setDraft] = useState<Record<string, string>>({});

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

  // ===== 价格同步（内置默认表 diff 提示）=====
  // updateCount：待应用的差异总数（用于按钮红点）；diffPanel：是否展开差异面板
  const [updateCount, setUpdateCount] = useState(0);
  const [diffPanel, setDiffPanel] = useState(false);
  const [diff, setDiff] = useState<PricingDiff | null>(null);
  // 用户勾选的项 key：`${model_id}|${currency}`（区分 cny/usd）
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);

  // ===== Cursor 配置 =====
  const [cursorCfg, setCursorCfg] = useState<CursorConfig>({
    cookie_source: "auto",
    cookie_header: "",
    usd_cny_rate: 7.2,
  });
  const [savingCursor, setSavingCursor] = useState(false);
  const [cursorSavedFlash, setCursorSavedFlash] = useState(false);
  const [cursorTesting, setCursorTesting] = useState(false);
  const [cursorTestResult, setCursorTestResult] = useState<string | null>(null);
  const [cursorDebugInfo, setCursorDebugInfo] = useState<string | null>(null);
  const [cursorDebugging, setCursorDebugging] = useState(false);

  useEffect(() => {
    Promise.all([
      fetchPricing(),
      fetchModels(),
      fetchQuotaConfig(),
      getShortcutConfig(),
      getCursorConfig(),
    ])
      .then(([p, m, q, s, cc]) => {
        setPricing(p);
        setModels(m);
        setQuotaCfg(q);
        setTokenDraft(q.token);
        setShortcutCfg(s);
        setShortcutDraft(s.accelerator);
        setCursorCfg(cc);
      })
      .catch((e) => setError(String(e)));
  }, []);

  // 进入价格设置时静默检查一次更新，有差异则在按钮显示红点（不弹窗打扰）。
  // cacheOnly=true：只读 24h 内磁盘缓存、不联网（首次无缓存时用内置表），保证进面板秒回不卡。
  // 需要最新价时点「检查更新」按钮（force 联网刷新缓存）。
  useEffect(() => {
    checkPricingUpdates(false, true)
      .then((d) => {
        const n = d.new_models.length + d.changed.length;
        setUpdateCount(n);
        setDiff(d);
        // 默认勾选：新增模型全勾，变动项默认不勾（保护用户自定义）
        const sel = new Set<string>();
        d.new_models.forEach(
          (i) => sel.add(`${i.model_id}|${i.currency}`)
        );
        setSelected(sel);
      })
      .catch(() => {});
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

  // 切换某条差异的勾选状态
  const toggleSelect = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // 应用勾选的价格更新：把勾选项合并进 pricing 并保存，绝不自动覆盖未勾选项
  const handleApplyUpdates = async () => {
    if (!diff) return;
    setApplying(true);
    setError(null);
    try {
      const all = [...diff.new_models, ...diff.changed];
      const items: ApplyPriceItem[] = all
        .filter((i) => selected.has(`${i.model_id}|${i.currency}`))
        .map((i) => ({
          model_id: i.model_id,
          currency: i.currency,
          price: i.default,
        }));
      if (items.length === 0) {
        setDiffPanel(false);
        return;
      }
      const updated = await applyPricingUpdates(items);
      setPricing(updated);
      setDraft({});
      // 重新检查差异
      const d = await checkPricingUpdates();
      const n = d.new_models.length + d.changed.length;
      setDiff(d);
      setUpdateCount(n);
      setSelected(new Set(d.new_models.map((i) => `${i.model_id}|${i.currency}`)));
      if (n === 0) setDiffPanel(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  };

  // 手动点「检查更新」按钮：强制重新拉取
  const handleCheckUpdates = async () => {
    setError(null);
    try {
      const d = await checkPricingUpdates(true);
      setDiff(d);
      setUpdateCount(d.new_models.length + d.changed.length);
      setSelected(new Set(d.new_models.map((i) => `${i.model_id}|${i.currency}`)));
      setDiffPanel(true);
    } catch (e) {
      setError(String(e));
    }
  };

  // 把草稿字符串解析回数字。非法/空 → 0。
  const parseDraft = (val: string): number => {
    const t = val.trim();
    if (t === "" || t === "." || t === "-") return 0;
    const n = parseFloat(t);
    return isNaN(n) ? 0 : n;
  };

  // 某输入框失焦：把草稿写回 pricing 数值
  const commitDraft = (modelId: string, key: keyof ModelPrice) => {
    const dk = `${modelId}|${key}`;
    const raw = draft[dk];
    if (raw === undefined) return; // 没编辑过，跳过
    const num = parseDraft(raw);
    setPricing((prev) => {
      if (!prev) return prev;
      const cur = prev[currency] ?? {};
      return {
        ...prev,
        [currency]: {
          ...cur,
          [modelId]: {
            ...(cur[modelId] ?? { input: 0, output: 0, cache_read: 0 }),
            [key]: num,
          },
        },
      };
    });
  };

  const handleSave = async () => {
    if (!pricing) return;
    // 保存前把所有未 commit 的草稿落盘
    const merged = { ...pricing };
    const cur = { ...(merged[currency] ?? {}) };
    for (const [dk, raw] of Object.entries(draft)) {
      const [modelId, key] = dk.split("|") as [string, keyof ModelPrice];
      cur[modelId] = {
        ...(cur[modelId] ?? { input: 0, output: 0, cache_read: 0 }),
        [key]: parseDraft(raw),
      };
    }
    merged[currency] = cur;

    setSaving(true);
    setError(null);
    try {
      await savePricing(merged);
      setPricing(merged);
      setDraft({});
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  if (!pricing) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-slate-700/55">
        加载中…
      </div>
    );
  }

  // 合并：数据库里的模型 + 价格表里手动加的模型
  const modelIds = Array.from(
    new Set([
      ...models.map((m) => m.model_id),
      ...Object.keys(pricing.cny),
      ...Object.keys(pricing.usd),
    ])
  ).sort();

  const symbol = currency === "cny" ? "¥" : "$";

  return (
    // 整页单一滚动：配置区 + 模型列表一起滚，避免配置区撑爆固定高度后下方被截断
    <div className="h-full overflow-y-auto">
      {/* 顶部 */}
      <div className="px-3.5 py-2.5 border-b border-slate-900/10">
        <div className="flex items-center justify-between mb-2">
          <button
            onClick={onBack}
            className="text-xs text-slate-700/60 hover:text-sky-600 transition-colors"
          >
            ← 返回
          </button>
          <h1 className="text-[13px] font-semibold text-slate-900/90">价格设置</h1>
          <button
            onClick={handleSave}
            disabled={saving}
            className="text-xs px-2.5 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
          >
            {saving ? "保存中…" : savedFlash ? "已保存 ✓" : "保存"}
          </button>
        </div>
        {/* 货币切换 */}
        <div className="flex gap-1">
          {(["cny", "usd"] as Currency[]).map((c) => (
            <button
              key={c}
              onClick={() => {
                onCurrencyChange(c);
                setDraft({});
              }}
              className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                currency === c
                  ? "bg-sky-500 text-white"
                  : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10 hover:text-slate-900/80"
              }`}
            >
              {c === "cny" ? "¥ 人民币" : "$ 美元"}
            </button>
          ))}
        </div>
        <p className="text-[10px] text-slate-700/50 mt-1.5">
          单位：{symbol}/百万 token。只填需要计费的模型即可。
        </p>

        {/* ===== 价格同步（内置默认表 diff 提示，不自动覆盖）===== */}
        <div className="mt-2 flex items-center justify-between">
          <button
            onClick={handleCheckUpdates}
            className="relative text-[11px] px-2 py-0.5 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 hover:text-slate-900/90 transition-colors"
          >
            🔄 检查价格更新
            {updateCount > 0 && (
              <span className="absolute -top-1 -right-1 min-w-[14px] h-[14px] px-1 flex items-center justify-center rounded-full bg-rose-500 text-white text-[9px] leading-none">
                {updateCount}
              </span>
            )}
          </button>
          {diff && updateCount === 0 && !diffPanel && (
            <span
              className={`text-[10px] ${
                diff.missing.length > 0
                  ? "text-amber-600/90"
                  : "text-emerald-600/80"
              }`}
            >
              {diff.missing.length > 0
                ? `${diff.missing.length} 个模型未配价`
                : "已是最新 ✓"}
            </span>
          )}
        </div>

        {/* 差异面板：展开时显示可勾选的新增/变动项 */}
        {diffPanel && diff && (
          <div className="mt-2 rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
            <div className="flex items-center justify-between mb-1.5">
              <div className="text-[11px] font-medium text-slate-900/85">
                价格更新
                {diff.source === "models.dev" ? (
                  <span className="ml-1.5 px-1 py-0 rounded text-[8px] font-medium bg-sky-500/15 text-sky-700">
                    models.dev 实时
                  </span>
                ) : (
                  <span className="ml-1.5 px-1 py-0 rounded text-[8px] font-medium bg-slate-900/10 text-slate-700/60">
                    内置表（离线{diff.version ? ` v${diff.version}` : ""}）
                  </span>
                )}
              </div>
            </div>
            <p className="text-[9px] text-slate-700/50 mb-2 leading-relaxed">
              {diff.source === "models.dev"
                ? "来自 models.dev 的智谱国际站 USD 指导价，CNY 按汇率换算。勾选后点「应用选中」才会写入。"
                : "models.dev 不可达，已回退内置参考表。勾选后点「应用选中」才会写入。"}
              新增项默认勾选，变动项默认不勾（保护你的自定义）。
            </p>
            {/* 无价格模型警示：实际在用但两边都没价格，花费按 0 计 */}
            {diff.missing.length > 0 && (
              <div className="mb-2 rounded-md bg-amber-500/10 border border-amber-500/20 px-2 py-1.5">
                <div className="text-[9px] text-amber-700/90 font-medium">
                  以下模型实际在用但未配置价格（花费按 0 计）：
                </div>
                <div className="text-[9px] text-slate-700/60 mt-0.5 num leading-relaxed break-all">
                  {diff.missing.join("、")}
                </div>
                <div className="text-[8px] text-slate-700/40 mt-0.5">
                  请在下方模型列表中手动补价
                </div>
              </div>
            )}
            <div className="space-y-1 max-h-44 overflow-y-auto">
              {/* 新增模型 */}
              {diff.new_models.map((i) => {
                const key = `${i.model_id}|${i.currency}`;
                return (
                  <label
                    key={`new-${key}`}
                    className="flex items-center gap-2 text-[10px] py-0.5 cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(key)}
                      onChange={() => toggleSelect(key)}
                      className="accent-sky-500 w-3 h-3"
                    />
                    <span className="text-emerald-600/90">新增</span>
                    <span className="font-medium text-slate-900/85 truncate">
                      {i.model_id}
                    </span>
                    <span className="text-slate-700/40">{i.currency.toUpperCase()}</span>
                    <span className="ml-auto text-slate-700/55 num">
                      {i.default.input}/{i.default.output}/{i.default.cache_read}
                    </span>
                  </label>
                );
              })}
              {/* 价格变动 */}
              {diff.changed.map((i) => {
                const key = `${i.model_id}|${i.currency}`;
                const u = i.user;
                return (
                  <label
                    key={`chg-${key}`}
                    className="flex items-center gap-2 text-[10px] py-0.5 cursor-pointer"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(key)}
                      onChange={() => toggleSelect(key)}
                      className="accent-sky-500 w-3 h-3"
                    />
                    <span className="text-amber-600/90">变动</span>
                    <span className="font-medium text-slate-900/85 truncate">
                      {i.model_id}
                    </span>
                    <span className="text-slate-700/40">{i.currency.toUpperCase()}</span>
                    <span className="ml-auto text-slate-700/55 num">
                      {u
                        ? `${u.input}/${u.output}/${u.cache_read}`
                        : "—"}
                      <span className="text-slate-700/35 mx-0.5">→</span>
                      <span className="text-sky-600/90">
                        {i.default.input}/{i.default.output}/{i.default.cache_read}
                      </span>
                    </span>
                  </label>
                );
              })}
              {diff.new_models.length === 0 && diff.changed.length === 0 && (
                <div className="text-[10px] text-slate-700/50 text-center py-2">
                  无差异，价格已是最新
                </div>
              )}
            </div>
            <div className="flex items-center justify-between mt-2">
              <button
                onClick={() => setDiffPanel(false)}
                className="text-[10px] text-slate-700/50 hover:text-slate-900/70 transition-colors"
              >
                收起
              </button>
              <button
                onClick={handleApplyUpdates}
                disabled={applying || selected.size === 0}
                className="text-[10px] px-2 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
              >
                {applying
                  ? "应用中…"
                  : `应用选中${selected.size > 0 ? ` (${selected.size})` : ""}`}
              </button>
            </div>
          </div>
        )}

        {/* ===== Coding Plan 额度查询配置 ===== */}
        <div className="mt-3 rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
          <div className="flex items-center justify-between mb-1.5">
            <span className="text-[11px] font-medium text-slate-900/85">
              Coding Plan 额度监控
            </span>
            <button
              onClick={handleSaveQuota}
              disabled={savingQuota}
              className="text-[10px] px-2 py-0.5 rounded-md bg-violet-500 text-white hover:bg-violet-600 disabled:opacity-40 transition-colors"
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
            <div className="flex items-center rounded-md bg-slate-900/5 border border-slate-900/10 focus-within:border-violet-400/60 focus-within:ring-1 focus-within:ring-violet-400/40 transition-colors">
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
                      ? "bg-violet-500 text-white"
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
        <div className="mt-2 rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
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
                disabled={cursorDebugging}
                className="text-[10px] px-2 py-0.5 rounded-md bg-slate-600/80 text-white hover:bg-slate-700 disabled:opacity-40 transition-colors"
              >
                {cursorDebugging ? "诊断中…" : "诊断"}
              </button>
              <button
                onClick={handleTestCursor}
                disabled={cursorTesting}
                className="text-[10px] px-2 py-0.5 rounded-md bg-violet-500/90 text-white hover:bg-violet-600 disabled:opacity-40 transition-colors"
              >
                {cursorTesting ? "测试中…" : "测试连接"}
              </button>
              <button
                onClick={handleSaveCursor}
                disabled={savingCursor}
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
                      ? "bg-violet-500/20 text-violet-700"
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
              className="w-full px-2 py-1 rounded-md bg-white/60 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-violet-400/60 mb-1.5"
            />
          )}

          {/* 汇率 */}
          <div className="flex items-center gap-2 mb-1">
            <span className="text-[10px] text-slate-700/60 w-12 shrink-0">
              USD→CNY
            </span>
            <input
              type="number"
              step="0.1"
              min="0.1"
              value={cursorCfg.usd_cny_rate}
              onChange={(e) => {
                const v = parseFloat(e.target.value);
                setCursorCfg({
                  ...cursorCfg,
                  // 拒绝 0 / 负数 / NaN，兜底默认汇率
                  usd_cny_rate: v > 0 ? v : 7.2,
                });
              }}
              className="num w-20 px-2 py-0.5 rounded-md bg-white/60 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-violet-400/60"
            />
            <span className="text-[9px] text-slate-700/45">
              汇总页合并花费换算用
            </span>
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
          <div className="mt-2 rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
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
                    className="accent-amber-500 w-3 h-3"
                  />
                  启用
                </label>
                <button
                  onClick={handleSaveShortcut}
                  disabled={savingShortcut}
                  className="text-[10px] px-2 py-0.5 rounded-md bg-amber-500 text-white hover:bg-amber-600 disabled:opacity-40 transition-colors"
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
            <div className="flex items-center rounded-md bg-slate-900/5 border border-slate-900/10 focus-within:border-amber-400/60 focus-within:ring-1 focus-within:ring-amber-400/40 transition-colors">
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

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {/* 模型列表 */}
      <div className="px-3.5 py-2.5">
        <div className="space-y-2">
          {/* 高峰期倍率设置 */}
          <PeakConfigEditor />

          {modelIds.map((id) => {
            const price =
              pricing[currency][id] ?? {
                input: 0,
                output: 0,
                cache_read: 0,
              };
            return (
              <div
                key={id}
                className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5"
              >
                <div className="text-xs font-medium text-slate-900/90 mb-2">
                  {id}
                </div>
                <div className="grid grid-cols-3 gap-1.5">
                  {FIELDS.map((f) => {
                    const dk = `${id}|${f.key}`;
                    const draftVal = draft[dk];
                    const shown =
                      draftVal !== undefined
                        ? draftVal
                        : price[f.key]
                        ? String(price[f.key])
                        : "";
                    return (
                      <label
                        key={f.key}
                        className="flex flex-col gap-0.5 text-[10px]"
                      >
                        <span className="text-slate-700/55">{f.label}</span>
                        <div className="flex items-center rounded-md bg-slate-900/5 border border-slate-900/10 focus-within:border-sky-400/60 focus-within:ring-1 focus-within:ring-sky-400/40 transition-colors">
                          <span className="text-slate-700/50 pl-1.5">{symbol}</span>
                          <input
                            type="text"
                            inputMode="decimal"
                            value={shown}
                            placeholder="0"
                            onChange={(e) => {
                              // 只允许数字和小数点
                              const v = e.target.value.replace(/[^\d.]/g, "");
                              setDraft((d) => ({ ...d, [dk]: v }));
                            }}
                            onBlur={() => commitDraft(id, f.key)}
                            className="num w-full px-1.5 py-1 text-right bg-transparent text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none"
                          />
                        </div>
                      </label>
                    );
                  })}
                </div>
              </div>
            );
          })}
          {modelIds.length === 0 && (
            <div className="text-center text-xs text-slate-700/50 py-8">
              暂无模型数据。请确认 ZCode 已产生使用记录。
            </div>
          )}
        </div>
      </div>
    </div>
  );
}

/** 周几名称（与 weekday_mask 的 bit 对应：bit0=周日） */
const WEEKDAY_LABELS = ["日", "一", "二", "三", "四", "五", "六"];

/** 高峰期倍率设置卡片：独立加载/编辑/保存 */
function PeakConfigEditor() {
  const [cfg, setCfg] = useState<PeakConfig | null>(null);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [switching, setSwitching] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getPeakConfig()
      .then(setCfg)
      .catch((e) => setError(String(e)));
  }, []);

  const update = (next: PeakConfig) => setCfg(next);

  const handleSave = async () => {
    if (!cfg) return;
    setSaving(true);
    setError(null);
    try {
      await setPeakConfig(cfg);
      setSavedFlash(true);
      setTimeout(() => setSavedFlash(false), 1500);
    } catch (e) {
      setError(String(e));
    } finally {
      setSaving(false);
    }
  };

  // 切换订阅类型：后端重置该类型默认时段（保留 zcode_discount）
  const handleSwitchPlan = async (plan: PlanType) => {
    if (!cfg || cfg.plan_type === plan) return;
    setSwitching(true);
    setError(null);
    try {
      const next = await setPlanType(plan);
      setCfg(next);
    } catch (e) {
      setError(String(e));
    } finally {
      setSwitching(false);
    }
  };

  if (!cfg) {
    return (
      <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5 text-center text-[10px] text-slate-700/50">
        加载高峰期配置…
      </div>
    );
  }

  const noPlan = cfg.plan_type === null;

  return (
    <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-2">
        <span className="text-[11px] font-medium text-slate-900/85">
          高峰期倍率设置
        </span>
        <button
          onClick={handleSave}
          disabled={saving || noPlan}
          className="text-[10px] px-2 py-0.5 rounded-md bg-violet-500 text-white hover:bg-violet-600 disabled:opacity-40 transition-colors"
          title={noPlan ? "请先选择订阅类型" : undefined}
        >
          {saving ? "保存中…" : savedFlash ? "已保存 ✓" : "保存"}
        </button>
      </div>

      {/* 订阅类型选择（必选，否则无法折算） */}
      <div className="mb-2">
        <div className="text-[10px] text-slate-700/55 mb-1">订阅类型</div>
        <div className="flex gap-1">
          {(["v2", "v3"] as PlanType[]).map((p) => (
            <button
              key={p}
              onClick={() => handleSwitchPlan(p)}
              disabled={switching}
              className={`flex-1 px-2 py-1 rounded-md text-[10px] transition-colors ${
                cfg.plan_type === p
                  ? "bg-violet-500 text-white"
                  : "bg-slate-900/8 text-slate-700/65 hover:bg-slate-900/15"
              }`}
            >
              {p === "v2" ? "V2 按请求倍率" : "V3 按积分"}
            </button>
          ))}
        </div>
        {noPlan && (
          <p className="text-[9px] text-amber-600/80 mt-1">
            请选择订阅类型以启用额度折算
          </p>
        )}
      </div>

      {/* ZCode 150% 提额优惠开关 */}
      <label className="flex items-center gap-1.5 mb-2 cursor-pointer">
        <input
          type="checkbox"
          checked={cfg.zcode_discount}
          onChange={(e) =>
            update({ ...cfg, zcode_discount: e.target.checked })
          }
          className="w-2.5 h-2.5 accent-violet-500"
        />
        <span className="text-[10px] text-slate-700/65">
          ZCode 150% 提额优惠（全周期 ×0.67）
        </span>
      </label>

      {/* 启用开关 */}
      <label className="flex items-center gap-1.5 mb-2 cursor-pointer">
        <input
          type="checkbox"
          checked={cfg.enabled}
          onChange={(e) => update({ ...cfg, enabled: e.target.checked })}
          className="w-2.5 h-2.5 accent-violet-500"
        />
        <span className="text-[10px] text-slate-700/65">启用折算</span>
      </label>

      {/* 规则说明（根据订阅类型） */}
      {!noPlan && (
        <p className="text-[9px] text-slate-700/45 mb-2 leading-relaxed">
          {cfg.plan_type === "v2"
            ? "V2：消耗 = token × 时段倍率（高峰3×/非高峰1×），周末全天非高峰。"
            : "V3：消耗 = 积分 = (input×系数 + cache×系数 + output×系数)/10000 × 时段倍率（高峰1.0/非高峰0.5）。"}
          {cfg.zcode_discount ? " 已启用 ZCode ×0.67 优惠。" : ""}
        </p>
      )}

      {/* 时段列表（未选订阅类型时隐藏） */}
      {!noPlan && (
        <>
          <div className="space-y-1.5">
            {cfg.segments.map((seg, idx) => (
              <div
                key={idx}
                className="rounded-md bg-white/30 border border-slate-900/8 p-1.5"
              >
                <div className="flex items-center gap-1.5 mb-1">
                  <input
                    type="time"
                    value={seg.start}
                    onChange={(e) =>
                      updateSegment(cfg, idx, { start: e.target.value }, update)
                    }
                    className="num px-1 py-0.5 rounded text-[10px] bg-slate-900/5 border border-slate-900/10 focus:outline-none focus:border-violet-400/60 text-slate-900/85"
                  />
                  <span className="text-[9px] text-slate-700/40">-</span>
                  <input
                    type="time"
                    value={seg.end}
                    onChange={(e) =>
                      updateSegment(cfg, idx, { end: e.target.value }, update)
                    }
                    className="num px-1 py-0.5 rounded text-[10px] bg-slate-900/5 border border-slate-900/10 focus:outline-none focus:border-violet-400/60 text-slate-900/85"
                  />
                  <span className="text-[9px] text-slate-700/45 shrink-0">倍率</span>
                  <input
                    type="number"
                    step="0.1"
                    min="0"
                    value={seg.multiplier}
                    onChange={(e) =>
                      updateSegment(
                        cfg,
                        idx,
                        { multiplier: parseFloat(e.target.value) || 0 },
                        update
                      )
                    }
                    className="num w-12 px-1 py-0.5 rounded text-[10px] bg-slate-900/5 border border-slate-900/10 focus:outline-none focus:border-violet-400/60 text-slate-900/85"
                  />
                  <button
                    onClick={() =>
                      update({
                        ...cfg,
                        segments: cfg.segments.filter((_, i) => i !== idx),
                      })
                    }
                    className="ml-auto text-[10px] text-slate-700/35 hover:text-red-500/70 transition-colors shrink-0"
                    title="删除时段"
                  >
                    ✕
                  </button>
                </div>
                <div className="flex items-center gap-1">
                  {WEEKDAY_LABELS.map((lbl, bit) => {
                    const hit = ((seg.weekday_mask >> bit) & 1) === 1;
                    return (
                      <button
                        key={bit}
                        onClick={() =>
                          updateSegment(
                            cfg,
                            idx,
                            { weekday_mask: seg.weekday_mask ^ (1 << bit) },
                            update
                          )
                        }
                        className={`w-5 h-5 rounded text-[9px] flex items-center justify-center transition-colors ${
                          hit
                            ? "bg-violet-500 text-white"
                            : "bg-slate-900/8 text-slate-700/40 hover:bg-slate-900/15"
                        }`}
                      >
                        {lbl}
                      </button>
                    );
                  })}
                </div>
              </div>
            ))}
          </div>

          <div className="flex items-center justify-between mt-1.5">
            <button
              onClick={() =>
                update({
                  ...cfg,
                  segments: [
                    ...cfg.segments,
                    {
                      start: "00:00",
                      end: "23:59",
                      multiplier: 1.0,
                      weekday_mask: MASK_WEEKDAY,
                    } as PeakSegment,
                  ],
                })
              }
              className="text-[10px] text-violet-600 hover:text-violet-700 transition-colors"
            >
              + 添加时段
            </button>
            <span className="text-[9px] text-slate-700/40">
              未匹配时段按 1.0 倍
            </span>
          </div>
        </>
      )}
      {error && (
        <div className="mt-1.5 text-[9px] text-red-600/80">{error}</div>
      )}
    </div>
  );
}

/** 更新某个时段的字段 */
function updateSegment(
  cfg: PeakConfig,
  idx: number,
  patch: Partial<PeakSegment>,
  commit: (next: PeakConfig) => void
) {
  const segments = cfg.segments.map((s, i) =>
    i === idx ? { ...s, ...patch } : s
  );
  commit({ ...cfg, segments });
}

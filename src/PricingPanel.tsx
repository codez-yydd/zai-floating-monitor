import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  ApplyPriceItem,
  Currency,
  CursorConfig,
  ModelInfo,
  ModelPrice,
  PricingConfig,
  PricingDiff,
  QuotaConfig,
  QuotaEndpoint,
  ShortcutConfig,
} from "./types";
import {
  applyPricingUpdates,
  checkPricingUpdates,
  fetchFxRate,
  fetchModels,
  fetchPricing,
  fetchQuotaConfig,
  getCursorConfig,
  getShortcutConfig,
  savePricing,
  saveQuotaConfig,
  setCursorConfig,
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

/// 价格数字显示格式化：6 位有效数字，去掉浮点乘法尾巴（0.39999999999999 → 0.4）
function fmtPrice(n: number): string {
  return String(Number(n.toPrecision(6)));
}

/// 汇率最近获取时间的显示格式：MM-DD HH:mm（本地时区）
function fmtFxTime(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

/// 价格三元组（输入/输出/缓存）显示
function fmtTriplet(p: ModelPrice): string {
  return `${fmtPrice(p.input)}/${fmtPrice(p.output)}/${fmtPrice(p.cache_read)}`;
}

/// 无价格模型的占位（复用同一引用，保证 memo 的模型卡片不会因 fallback 新对象而失效）
const ZERO_PRICE: ModelPrice = { input: 0, output: 0, cache_read: 0 };

export function PricingPanel({ currency, onCurrencyChange, onBack }: Props) {
  const [pricing, setPricing] = useState<PricingConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // 手动检查联网中（Force 拉取需要几秒，按钮显示 loading 防重复点击）
  const [checking, setChecking] = useState(false);
  // 草稿：把每个输入框的编辑值作为字符串暂存，避免小数点输入被 parseFloat 吞掉。
  // key = `${modelId}|${field}`
  const [draft, setDraft] = useState<Record<string, string>>({});
  // draft 的 ref 镜像：onBlur 提交时读最新值，避免把回调绑进 draft 依赖导致整列表重渲染。
  // 用 useLayoutEffect 同步：渲染后立刻刷新，保证任何后续事件（onBlur）读到的是最新草稿
  const draftRef = useRef<Record<string, string>>({});
  useLayoutEffect(() => {
    draftRef.current = draft;
  }, [draft]);

  // 草稿变更（稳定引用）：单个输入框的键入只重渲染对应的 memo 模型卡片，而非整个列表
  const handleDraftChange = useCallback(
    (id: string, key: keyof ModelPrice, v: string) => {
      setDraft((d) => ({ ...d, [`${id}|${key}`]: v }));
    },
    []
  );

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
  // 默认（LocalFirst）：读本地缓存对比（秒回，不管缓存新旧），完全无缓存才联网兜底。
  // 缓存每天由后台定时任务自动联网刷新一次；手动拉最新数据点「更新」按钮。
  useEffect(() => {
    checkPricingUpdates()
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

  // 对比并展示差异。force=false 用本地缓存（秒回）；force=true 联网刷新缓存后对比。
  const runDiff = async (force: boolean) => {
    setError(null);
    if (force) setChecking(true);
    try {
      const d = await checkPricingUpdates(force);
      setDiff(d);
      setUpdateCount(d.new_models.length + d.changed.length);
      setSelected(new Set(d.new_models.map((i) => `${i.model_id}|${i.currency}`)));
      setDiffPanel(true);
    } catch (e) {
      setError(String(e));
    } finally {
      if (force) setChecking(false);
    }
  };

  // 「检查价格更新」：用本地缓存对比差异（秒回，不联网）
  const handleCheckUpdates = () => runDiff(false);

  // 「更新」：手动联网拉取 models.dev 最新数据刷新缓存，完成后展示新对比
  const handleRefreshPrices = () => runDiff(true);

  // 把草稿字符串解析回数字。非法/空 → 0。
  const parseDraft = (val: string): number => {
    const t = val.trim();
    if (t === "" || t === "." || t === "-") return 0;
    const n = parseFloat(t);
    return isNaN(n) ? 0 : n;
  };

  // 某输入框失焦：把草稿写回 pricing 数值（读 draftRef 最新值，引用随 currency 重建）
  const commitDraft = useCallback(
    (modelId: string, key: keyof ModelPrice) => {
      const raw = draftRef.current[`${modelId}|${key}`];
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
              ...(cur[modelId] ?? ZERO_PRICE),
              [key]: num,
            },
          },
        };
      });
    },
    [currency]
  );

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

  // 合并：数据库里的模型 + 价格表里手动加的模型（记忆化，避免每次渲染重算集合与排序）。
  // 必须位于下方 if (!pricing) 早退之前：条件早退会跳过 hooks，导致前后渲染 hooks 数不一致而崩溃
  const modelIds = useMemo(
    () =>
      pricing
        ? Array.from(
            new Set([
              ...models.map((m) => m.model_id),
              ...Object.keys(pricing.cny),
              ...Object.keys(pricing.usd),
            ])
          ).sort()
        : [],
    [models, pricing]
  );

  if (!pricing) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-slate-700/55">
        加载中…
      </div>
    );
  }

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

        {/* ===== 价格同步（models.dev 差异提示，不自动覆盖）===== */}
        <div className="mt-2 flex items-center justify-between">
          <div className="flex items-center gap-1.5">
            <button
              onClick={handleCheckUpdates}
              title="用本地缓存的价格数据对比差异（秒回，不联网）"
              className="relative text-[11px] px-2 py-0.5 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 hover:text-slate-900/90 transition-colors"
            >
              🔄 检查价格更新
              {updateCount > 0 && (
                <span className="absolute -top-1 -right-1 min-w-[14px] h-[14px] px-1 flex items-center justify-center rounded-full bg-rose-500 text-white text-[9px] leading-none">
                  {updateCount}
                </span>
              )}
            </button>
            <button
              onClick={handleRefreshPrices}
              disabled={checking}
              title="联网拉取 models.dev 最新价格并刷新本地缓存（后台每天也会自动更新一次）"
              className="text-[11px] px-2 py-0.5 rounded-md bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
            >
              {checking ? "⏳ 更新中…" : "⬇️ 更新"}
            </button>
          </div>
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
                ? "来自 models.dev 的全厂商模型 USD 参考价（你在 ZCode 里用到的任意厂商模型都会检测），CNY 按汇率换算；后台每天自动更新一次缓存，点「更新」可立即联网刷新。"
                : "models.dev 不可达，已回退内置参考表；点「更新」可重试联网。"}
              勾选后点「应用选中」才会写入；新增项默认勾选，变动项默认不勾（保护你的自定义）。价格三项依次为 输入/输出/缓存（每百万 token）。
            </p>
            {/* 无价格模型警示：实际在用但两边都没价格，花费按 0 计 */}
            {diff.missing.length > 0 && (
              <div className="mb-2 rounded-md bg-amber-500/10 border border-amber-500/20 px-2 py-1.5">
                <div className="text-[9px] text-amber-700/90 font-medium">
                  以下 {diff.missing.length} 个模型实际在用但未配置价格（花费按 0 计）：
                </div>
                <ul className="mt-0.5 max-h-16 overflow-y-auto space-y-0.5">
                  {diff.missing.map((m) => (
                    <li key={m} className="text-[9px] text-slate-700/60 num break-words">
                      {m}
                    </li>
                  ))}
                </ul>
                <div className="text-[8px] text-slate-700/40 mt-0.5">
                  请在下方模型列表中手动补价
                </div>
              </div>
            )}
            <div className="space-y-1 max-h-52 overflow-y-auto">
              {/* 新增模型 */}
              {diff.new_models.map((i) => {
                const key = `${i.model_id}|${i.currency}`;
                const sym = i.currency === "cny" ? "¥" : "$";
                return (
                  <label
                    key={`new-${key}`}
                    className="flex items-start gap-2 py-1 px-1 rounded cursor-pointer hover:bg-white/50 transition-colors"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(key)}
                      onChange={() => toggleSelect(key)}
                      className="accent-sky-500 w-3 h-3 mt-[3px] shrink-0"
                    />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-baseline gap-1.5">
                        <span className="shrink-0 px-1 rounded bg-emerald-500/15 text-emerald-700 text-[8px] font-medium">
                          新增
                        </span>
                        <span className="font-medium text-slate-900/85 text-[10px] break-words">
                          {i.model_id}
                        </span>
                      </div>
                      <div className="mt-0.5 text-[10px] text-slate-700/60 num whitespace-nowrap">
                        {sym} {fmtTriplet(i.default)}
                      </div>
                    </div>
                  </label>
                );
              })}
              {/* 价格变动 */}
              {diff.changed.map((i) => {
                const key = `${i.model_id}|${i.currency}`;
                const u = i.user;
                const sym = i.currency === "cny" ? "¥" : "$";
                return (
                  <label
                    key={`chg-${key}`}
                    className="flex items-start gap-2 py-1 px-1 rounded cursor-pointer hover:bg-white/50 transition-colors"
                  >
                    <input
                      type="checkbox"
                      checked={selected.has(key)}
                      onChange={() => toggleSelect(key)}
                      className="accent-sky-500 w-3 h-3 mt-[3px] shrink-0"
                    />
                    <div className="min-w-0 flex-1">
                      <div className="flex items-baseline gap-1.5">
                        <span className="shrink-0 px-1 rounded bg-amber-500/15 text-amber-700 text-[8px] font-medium">
                          变动
                        </span>
                        <span className="font-medium text-slate-900/85 text-[10px] break-words">
                          {i.model_id}
                        </span>
                      </div>
                      <div className="mt-0.5 text-[10px] num leading-relaxed">
                        <span className="text-slate-700/40 text-[9px]">旧 </span>
                        <span className="text-slate-700/55 whitespace-nowrap">
                          {u ? `${sym} ${fmtTriplet(u)}` : "未配价"}
                        </span>
                        <span className="text-slate-700/35 mx-1">→</span>
                        <span className="text-sky-600/90 font-medium whitespace-nowrap">
                          {sym} {fmtTriplet(i.default)}
                        </span>
                      </div>
                    </div>
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
                value={cursorCfg.usd_cny_rate}
                readOnly={cursorCfg.fx_rate_auto}
                title={
                  cursorCfg.fx_rate_auto
                    ? "自动更新中，取消勾选可手动输入"
                    : undefined
                }
                onChange={(e) => {
                  const v = parseFloat(e.target.value);
                  setCursorCfg({
                    ...cursorCfg,
                    // 拒绝 0 / 负数 / NaN，兜底默认汇率
                    usd_cny_rate: v > 0 ? v : 7.2,
                  });
                }}
                className={`num w-20 px-2 py-0.5 rounded-md bg-white/60 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-violet-400/60 ${
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
                  className="accent-violet-500 w-3 h-3"
                />
                每日自动更新汇率
              </label>
              <button
                onClick={() => handleFetchFxRate()}
                disabled={fxUpdating}
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
          {modelIds.map((id) => (
            <ModelPriceRow
              key={id}
              id={id}
              symbol={symbol}
              price={pricing[currency][id] ?? ZERO_PRICE}
              dIn={draft[`${id}|input`]}
              dOut={draft[`${id}|output`]}
              dCache={draft[`${id}|cache_read`]}
              onDraftChange={handleDraftChange}
              onCommit={commitDraft}
            />
          ))}
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

/** 单个模型的价格卡片（memo：草稿用三个原始类型 props 传递，
 *  键入时只重渲染这一张卡片而非整个模型列表） */
const ModelPriceRow = memo(function ModelPriceRow({
  id,
  symbol,
  price,
  dIn,
  dOut,
  dCache,
  onDraftChange,
  onCommit,
}: {
  id: string;
  symbol: string;
  price: ModelPrice;
  dIn: string | undefined;
  dOut: string | undefined;
  dCache: string | undefined;
  onDraftChange: (id: string, key: keyof ModelPrice, v: string) => void;
  onCommit: (id: string, key: keyof ModelPrice) => void;
}) {
  const drafts: Record<keyof ModelPrice, string | undefined> = {
    input: dIn,
    output: dOut,
    cache_read: dCache,
  };
  return (
    <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
      <div className="text-xs font-medium text-slate-900/90 mb-2">{id}</div>
      <div className="grid grid-cols-3 gap-1.5">
        {FIELDS.map((f) => {
          const draftVal = drafts[f.key];
          const shown =
            draftVal !== undefined
              ? draftVal
              : price[f.key]
                ? String(price[f.key])
                : "";
          return (
            <label key={f.key} className="flex flex-col gap-0.5 text-[10px]">
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
                    onDraftChange(id, f.key, v);
                  }}
                  onBlur={() => onCommit(id, f.key)}
                  className="num w-full px-1.5 py-1 text-right bg-transparent text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none"
                />
              </div>
            </label>
          );
        })}
      </div>
    </div>
  );
});

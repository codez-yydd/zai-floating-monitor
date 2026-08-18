import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type {
  ApplyPriceItem,
  Currency,
  ModelInfo,
  ModelPrice,
  PricingConfig,
  PricingDiff,
} from "./types";
import {
  applyPricingUpdates,
  checkPricingUpdates,
  fetchModels,
  fetchPricing,
  getCursorConfig,
  savePricing,
} from "./api";
import {
  PageShell,
  PageHeader,
  PageBody,
  SectionCard,
  PillGroup,
  PillButton,
  BtnPrimary,
  BtnSecondary,
  AlertBanner,
  LoadingState,
} from "./layout";

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

/// 价格三元组（输入/输出/缓存）显示
function fmtTriplet(p: ModelPrice): string {
  return `${fmtPrice(p.input)}/${fmtPrice(p.output)}/${fmtPrice(p.cache_read)}`;
}

/// 按汇率把美元价折算为人民币价（仅展示用，不落盘）
function scaleCny(p: ModelPrice, rate: number): ModelPrice {
  return {
    input: Number((p.input * rate).toPrecision(6)),
    output: Number((p.output * rate).toPrecision(6)),
    cache_read: Number((p.cache_read * rate).toPrecision(6)),
  };
}

/// 无价格模型的占位（复用同一引用，保证 memo 的模型卡片不会因 fallback 新对象而失效）
const ZERO_PRICE: ModelPrice = { input: 0, output: 0, cache_read: 0 };

export function PricingPanel({ currency, onCurrencyChange, onBack }: Props) {
  const [pricing, setPricing] = useState<PricingConfig | null>(null);
  const [models, setModels] = useState<ModelInfo[]>([]);
  const [saving, setSaving] = useState(false);
  const [savedFlash, setSavedFlash] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // USD→CNY 汇率：¥ 视图的折算展示用（价格只存美元，人民币一律实时折算）
  const [fxRate, setFxRate] = useState(7.2);
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

  // ===== 价格同步（内置参考表 diff 提示）=====
  // updateCount：待应用的差异总数（用于按钮红点）；diffPanel：是否展开差异面板
  const [updateCount, setUpdateCount] = useState(0);
  const [diffPanel, setDiffPanel] = useState(false);
  const [diff, setDiff] = useState<PricingDiff | null>(null);
  // 用户勾选的模型 id 集合（差异条目为模型级）
  const [selected, setSelected] = useState<Set<string>>(new Set());
  const [applying, setApplying] = useState(false);

  useEffect(() => {
    Promise.all([fetchPricing(), fetchModels()])
      .then(([p, m]) => {
        setPricing(p);
        setModels(m);
      })
      .catch((e) => setError(String(e)));
    // 汇率与 Cursor 设置共用同一来源（设置页可改、每日自动更新）
    getCursorConfig()
      .then((c) => {
        if (c.usd_cny_rate > 0) setFxRate(c.usd_cny_rate);
      })
      .catch(() => {});
  }, []);

  // 进入价格设置时静默检查一次差异，有差异则在按钮显示红点（纯本地对比，秒回不联网）。
  // 差异判定只看 USD 原始价（人民币按汇率折算展示，不参与判定）。
  useEffect(() => {
    checkPricingUpdates()
      .then((d) => {
        const n = d.new_models.length + d.changed.length;
        setUpdateCount(n);
        setDiff(d);
        // 默认勾选：新增模型全勾，变动项默认不勾（保护用户自定义）
        const sel = new Set<string>();
        d.new_models.forEach((i) => sel.add(i.model_id));
        setSelected(sel);
      })
      .catch(() => {});
  }, []);

  // 切换某条差异的勾选状态
  const toggleSelect = (key: string) => {
    setSelected((prev) => {
      const next = new Set(prev);
      if (next.has(key)) next.delete(key);
      else next.add(key);
      return next;
    });
  };

  // 应用勾选的价格更新：把勾选项的美元价合并进 pricing 并保存，绝不自动覆盖未勾选项
  const handleApplyUpdates = async () => {
    if (!diff) return;
    setApplying(true);
    setError(null);
    try {
      const all = [...diff.new_models, ...diff.changed];
      const items: ApplyPriceItem[] = all
        .filter((i) => selected.has(i.model_id))
        .map((i) => ({ model_id: i.model_id, currency: "usd" as const, price: i.default }));
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
      setSelected(new Set(d.new_models.map((i) => i.model_id)));
      if (n === 0) setDiffPanel(false);
    } catch (e) {
      setError(String(e));
    } finally {
      setApplying(false);
    }
  };

  // 对比并展示差异（纯本地对比内置参考表，无网络请求）
  const runDiff = async () => {
    setError(null);
    try {
      const d = await checkPricingUpdates();
      setDiff(d);
      setUpdateCount(d.new_models.length + d.changed.length);
      setSelected(new Set(d.new_models.map((i) => i.model_id)));
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

  // 某输入框失焦：把草稿写回 pricing 数值（读 draftRef 最新值）。只编辑美元价
  const commitDraft = useCallback(
    (modelId: string, key: keyof ModelPrice) => {
      const raw = draftRef.current[`${modelId}|${key}`];
      if (raw === undefined) return; // 没编辑过，跳过
      const num = parseDraft(raw);
      setPricing((prev) => {
        if (!prev) return prev;
        const cur = prev.usd ?? {};
        return {
          ...prev,
          usd: {
            ...cur,
            [modelId]: {
              ...(cur[modelId] ?? ZERO_PRICE),
              [key]: num,
            },
          },
        };
      });
    },
    []
  );

  const handleSave = async () => {
    if (!pricing) return;
    // 保存前把所有未 commit 的草稿落盘（只存美元价）
    const merged = { ...pricing };
    const cur = { ...(merged.usd ?? {}) };
    for (const [dk, raw] of Object.entries(draft)) {
      const [modelId, key] = dk.split("|") as [string, keyof ModelPrice];
      cur[modelId] = {
        ...(cur[modelId] ?? { input: 0, output: 0, cache_read: 0 }),
        [key]: parseDraft(raw),
      };
    }
    merged.usd = cur;

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
            new Set([...models.map((m) => m.model_id), ...Object.keys(pricing.usd)])
          ).sort()
        : [],
    [models, pricing]
  );

  if (!pricing) {
    return <LoadingState text="加载价格配置…" />;
  }

  return (
    <PageShell>
      <PageHeader
        title="价格设置"
        onBack={onBack}
        right={
          <BtnPrimary onClick={handleSave} disabled={saving}>
            {saving ? "保存中…" : savedFlash ? "已保存 ✓" : "保存"}
          </BtnPrimary>
        }
        subtitle={
          <>
            <PillGroup className="mt-0">
              {(["cny", "usd"] as Currency[]).map((c) => (
                <PillButton
                  key={c}
                  active={currency === c}
                  onClick={() => { onCurrencyChange(c); setDraft({}); }}
                >
                  {c === "cny" ? "¥ 人民币" : "$ 美元"}
                </PillButton>
              ))}
            </PillGroup>
            <p className="text-[10px] text-slate-500 mt-1.5">
              单位：$/百万 token。人民币按汇率
              <span className="num"> {fxRate} </span>
              自动折算。
            </p>
            <div className="mt-2 flex items-center justify-between">
              <BtnSecondary onClick={runDiff} className="relative">
                检查价格更新
                {updateCount > 0 && (
                  <span className="absolute -top-1 -right-1 min-w-[14px] h-[14px] px-1 flex items-center justify-center rounded-full bg-rose-500 text-white text-[9px] leading-none">
                    {updateCount}
                  </span>
                )}
              </BtnSecondary>
              {diff && updateCount === 0 && !diffPanel && (
                <span className={`text-[10px] ${diff.missing.length > 0 ? "text-amber-600" : "text-emerald-600"}`}>
                  {diff.missing.length > 0 ? `${diff.missing.length} 个模型未配价` : "已是最新 ✓"}
                </span>
              )}
            </div>
          </>
        }
      />

      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}

        {diffPanel && diff && (
          <SectionCard title={`价格更新 · 内置参考表${diff.version ? ` v${diff.version}` : ""}`}>
            <p className="text-[9px] text-slate-500 mb-2 leading-relaxed">
              参考价离线对比不联网。带 ≈ 标记的是变体名匹配的基础模型参考价，应用前请确认。
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
                const key = i.model_id;
                return (
                  <label
                    key={`new-${key}`}
                    className="flex items-start gap-2 py-1 px-1 rounded cursor-pointer hover:bg-surface/50 transition-colors"
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
                        {i.reference_id && (
                          <span
                            title={`参考价取自基础模型 ${i.reference_id}（同系变体），应用前请确认价格`}
                            className="shrink-0 px-1 rounded bg-amber-500/15 text-amber-700 text-[8px] font-medium"
                          >
                            ≈ {i.reference_id}
                          </span>
                        )}
                      </div>
                      <div className="mt-0.5 text-[10px] text-slate-700/60 num whitespace-nowrap">
                        $ {fmtTriplet(i.default)}
                        <span className="text-slate-700/40 ml-1.5">
                          ≈ ¥ {fmtTriplet(scaleCny(i.default, fxRate))}
                        </span>
                      </div>
                    </div>
                  </label>
                );
              })}
              {/* 价格变动（以 USD 参考价为准） */}
              {diff.changed.map((i) => {
                const key = i.model_id;
                const u = i.user;
                return (
                  <label
                    key={`chg-${key}`}
                    className="flex items-start gap-2 py-1 px-1 rounded cursor-pointer hover:bg-surface/50 transition-colors"
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
                          {u ? `$ ${fmtTriplet(u)}` : "未配价"}
                        </span>
                        <span className="text-slate-700/35 mx-1">→</span>
                        <span className="text-sky-600/90 font-medium whitespace-nowrap">
                          $ {fmtTriplet(i.default)}
                        </span>
                        <span className="text-slate-700/40 ml-1.5 whitespace-nowrap">
                          ≈ ¥ {fmtTriplet(scaleCny(i.default, fxRate))}
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
              <BtnSecondary onClick={() => setDiffPanel(false)}>收起</BtnSecondary>
              <BtnPrimary onClick={handleApplyUpdates} disabled={applying || selected.size === 0}>
                {applying ? "应用中…" : `应用选中${selected.size > 0 ? ` (${selected.size})` : ""}`}
              </BtnPrimary>
            </div>
          </SectionCard>
        )}

        {modelIds.map((id) => (
          <ModelPriceRow
            key={id}
            id={id}
            price={pricing.usd[id] ?? ZERO_PRICE}
            dIn={draft[`${id}|input`]}
            dOut={draft[`${id}|output`]}
            dCache={draft[`${id}|cache_read`]}
            onDraftChange={handleDraftChange}
            onCommit={commitDraft}
          />
        ))}
        {modelIds.length === 0 && (
          <div className="text-center text-xs text-slate-500 py-8">
            暂无模型数据。请确认 ZCode 已产生使用记录。
          </div>
        )}
      </PageBody>
    </PageShell>
  );
}

/** 单个模型的价格卡片（memo：草稿用三个原始类型 props 传递，
 *  键入时只重渲染这一张卡片而非整个模型列表）。表单恒定为美元编辑 */
const ModelPriceRow = memo(function ModelPriceRow({
  id,
  price,
  dIn,
  dOut,
  dCache,
  onDraftChange,
  onCommit,
}: {
  id: string;
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
    <div className="card-base rounded-2xl p-3">
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
              <div className="flex items-center rounded-md bg-surface/60 border border-slate-900/10 focus-within:border-sky-400/60 focus-within:ring-1 focus-within:ring-sky-400/40 transition-colors">
                <span className="text-slate-700/50 pl-1.5">$</span>
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

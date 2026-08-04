import { useEffect, useState } from "react";
import type { Currency, ModelInfo, ModelPrice, PricingConfig } from "./types";
import { fetchModels, fetchPricing, savePricing } from "./api";

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

  useEffect(() => {
    Promise.all([fetchPricing(), fetchModels()])
      .then(([p, m]) => {
        setPricing(p);
        setModels(m);
      })
      .catch((e) => setError(String(e)));
  }, []);

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
    <div className="flex flex-col h-full">
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
      </div>

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {/* 模型列表 */}
      <div className="flex-1 overflow-y-auto px-3.5 py-2.5">
        <div className="space-y-2">
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

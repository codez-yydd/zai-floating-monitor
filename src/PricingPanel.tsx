import { useEffect, useState } from "react";
import type {
  Currency,
  ModelInfo,
  ModelPrice,
  PeakConfig,
  PeakSegment,
  PlanType,
  PricingConfig,
  QuotaConfig,
  QuotaEndpoint,
} from "./types";
import { MASK_WEEKDAY } from "./types";
import {
  fetchModels,
  fetchPricing,
  fetchQuotaConfig,
  getPeakConfig,
  savePricing,
  saveQuotaConfig,
  setPeakConfig,
  setPlanType,
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

  useEffect(() => {
    Promise.all([fetchPricing(), fetchModels(), fetchQuotaConfig()])
      .then(([p, m, q]) => {
        setPricing(p);
        setModels(m);
        setQuotaCfg(q);
        setTokenDraft(q.token);
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
      </div>

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {/* 模型列表 */}
      <div className="flex-1 overflow-y-auto px-3.5 py-2.5">
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

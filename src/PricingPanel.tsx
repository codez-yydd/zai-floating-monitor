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
  syncModelsdevPricing,
} from "./api";
import { canonicalModelId, lookupPrice } from "./modelName";
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
import { useI18n, type MessageKey } from "./i18n";

interface Props {
  currency: Currency;
  onCurrencyChange: (c: Currency) => void;
  onBack: () => void;
}

// 模式 A：字段标签存词典键，渲染时查（ModelPriceRow 内 useI18n）
const FIELDS: { key: keyof ModelPrice; labelKey: MessageKey }[] = [
  { key: "input", labelKey: "common.input" },
  { key: "output", labelKey: "common.output" },
  { key: "cache_read", labelKey: "common.cache" },
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

/// models.dev 数据时间格式化（本地时区，YYYY-MM-DD HH:mm）
function fmtStamp(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}-${p(d.getMonth() + 1)}-${p(d.getDate())} ${p(d.getHours())}:${p(d.getMinutes())}`;
}

export function PricingPanel({ currency, onCurrencyChange, onBack }: Props) {
  const { t } = useI18n();
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
  // pricing 的 ref 镜像：删除/恢复是即时落盘动作（不走草稿+保存），回调以稳定引用
  // 绑进 memo 卡片与恢复面板，点击时经 ref 读最新配置，避免回调依赖 pricing
  const pricingRef = useRef<PricingConfig | null>(null);
  useLayoutEffect(() => {
    pricingRef.current = pricing;
  }, [pricing]);
  // 隐藏名单小面板的展开状态（列表底部「已隐藏 N 个模型」）
  const [hiddenOpen, setHiddenOpen] = useState(false);

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
  // 差异参考源：builtin = 编译期内置表（检查更新）；modelsdev = models.dev 在线官方价。
  // 两个来源互斥展示：点哪个按钮就用哪个结果覆盖当前 diff 状态
  const [diffSource, setDiffSource] = useState<"builtin" | "modelsdev">("builtin");
  // diffSource 的 ref 镜像（与 draftRef 同模式）：晚到的异步结果落地前校验当前来源，
  // 避免把回调绑进依赖导致重复执行
  const diffSourceRef = useRef<"builtin" | "modelsdev">("builtin");
  useLayoutEffect(() => {
    diffSourceRef.current = diffSource;
  }, [diffSource]);
  // models.dev 同步进行中（防重复点击）+ 最近一次结果的数据元信息
  const [syncing, setSyncing] = useState(false);
  const [modelsdevMeta, setModelsdevMeta] = useState<{
    fetchedAt: number;
    fromCache: boolean;
  } | null>(null);
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

  // 把某个来源的 diff 结果写入共享状态（更新差异数 + 默认勾选全部新增项，
  // 变动项默认不勾以保护用户自定义）
  const applyDiffState = (d: PricingDiff) => {
    setDiff(d);
    setUpdateCount(d.new_models.length + d.changed.length);
    setSelected(new Set(d.new_models.map((i) => i.model_id)));
  };

  // 进入价格设置时静默检查一次差异，有差异则在按钮显示红点（纯本地对比，秒回不联网）。
  // 差异判定只看 USD 原始价（人民币按汇率折算展示，不参与判定）。
  useEffect(() => {
    checkPricingUpdates()
      .then((d) => {
        // 内置检查内部含 codex 导入冷启动、可能晚到数秒：期间用户若已切到
        // models.dev 来源，丢弃这份过期结果，避免覆盖在线同步的 diff
        // 造成"来源标注 modelsdev、数据却是 builtin"的错位（红点也随之不更新）
        if (diffSourceRef.current !== "builtin") return;
        applyDiffState(d);
      })
      .catch(() => {});
    // eslint-disable-next-line react-hooks/exhaustive-deps
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
      // 重新检查差异：与当前展示的参考源保持一致（两种来源互斥展示，若应用
      // models.dev 价后改用内置表复查，会把刚应用的官方价误报为"变动"）
      if (diffSource === "modelsdev") {
        const r = await syncModelsdevPricing();
        setModelsdevMeta({ fetchedAt: r.fetched_at, fromCache: r.from_cache });
        applyDiffState(r.diff);
        if (r.diff.new_models.length + r.diff.changed.length === 0) setDiffPanel(false);
      } else {
        const d = await checkPricingUpdates();
        applyDiffState(d);
        if (d.new_models.length + d.changed.length === 0) setDiffPanel(false);
      }
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
      setDiffSource("builtin");
      setModelsdevMeta(null);
      applyDiffState(d);
      setDiffPanel(true);
    } catch (e) {
      setError(String(e));
    }
  };

  // 从 models.dev 在线同步官方厂商价格（网络失败自动降级本地缓存）。
  // 结果与内置表来源互斥：直接覆盖当前 diff 状态
  const runModelsdevSync = async () => {
    setError(null);
    setSyncing(true);
    try {
      const r = await syncModelsdevPricing();
      setDiffSource("modelsdev");
      setModelsdevMeta({ fetchedAt: r.fetched_at, fromCache: r.from_cache });
      applyDiffState(r.diff);
      setDiffPanel(true);
    } catch (e) {
      setError(`${String(e)}（${t("pricing.syncFailHint")}）`);
    } finally {
      setSyncing(false);
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
        // 表单按归一名去重只显示一行；写入时把同归一键（大小写/隐藏字符变体）
        // 的残留条目合并掉，避免残留条目被兜底查找优先命中，让编辑看起来不生效。
        // 合并域用 canonicalModelId（与表单行分组同域）：点号别名是两行可见条目，不在此折叠
        const target = canonicalModelId(modelId);
        const next: Record<string, ModelPrice> = {};
        for (const [k, v] of Object.entries(cur)) {
          if (k !== modelId && canonicalModelId(k) === target) continue;
          next[k] = v;
        }
        next[modelId] = {
          ...(cur[modelId] ?? lookupPrice(cur, modelId) ?? ZERO_PRICE),
          [key]: num,
        };
        return {
          ...prev,
          usd: next,
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
        // 与 commitDraft 同基底：优先沿用兜底命中的既有三元组，避免丢 output/cache_read
        ...(cur[modelId] ??
          lookupPrice(cur, modelId) ?? { input: 0, output: 0, cache_read: 0 }),
        [key]: parseDraft(raw),
      };
    }
    // 与 commitDraft 同域（canonicalModelId）：只合并本次被编辑展示键的
    // 大小写/隐藏字符残留写法；两者都未编辑的条目（含点号别名双行）保持原样，
    // 不做静默折叠，避免无感知地删除用户已配置的价格
    const edited = new Set(
      Object.keys(draft).map((dk) => dk.split("|")[0])
    );
    const consolidated: Record<string, ModelPrice> = {};
    for (const [k, v] of Object.entries(cur)) {
      const target = canonicalModelId(k);
      const kept = Object.keys(consolidated).find(
        (c) => canonicalModelId(c) === target
      );
      if (kept === undefined) {
        consolidated[k] = v;
      } else if (edited.has(k) && !edited.has(kept)) {
        consolidated[k] = v;
        delete consolidated[kept];
      }
    }
    merged.usd = consolidated;

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

  // 从列表移除模型（明确动作，即时落盘，不走草稿+保存的按钮模式）：
  // 1) 删 usd 价格条目（按归一键删该模型全部大小写/变体写法，与 commitDraft 合并域一致）；
  // 2) 记入隐藏名单（阻止它因数据库出现过再次出现在列表与差异提醒中）；
  // 3) 清该模型全部草稿（防止后续「保存」把已删模型经残留草稿写回）。
  // 落盘失败报错并回滚本地状态
  const handleRemove = useCallback(async (id: string) => {
    const prev = pricingRef.current;
    if (!prev) return;
    const target = canonicalModelId(id);
    const usd: Record<string, ModelPrice> = {};
    for (const [k, v] of Object.entries(prev.usd ?? {})) {
      if (canonicalModelId(k) === target) continue;
      usd[k] = v;
    }
    const next: PricingConfig = {
      ...prev,
      usd,
      hidden: [...new Set([...(prev.hidden ?? []), id])],
    };
    const prevDraft = draftRef.current;
    const nextDraft: Record<string, string> = {};
    for (const [dk, v] of Object.entries(prevDraft)) {
      // 草稿键前缀按归一匹配：同模型任何写法的残留草稿都不会再被 handleSave 写回
      if (canonicalModelId(dk.split("|")[0]) === target) continue;
      nextDraft[dk] = v;
    }
    setDraft(nextDraft);
    setPricing(next);
    setError(null);
    try {
      await savePricing(next);
    } catch (e) {
      setPricing(prev);
      setDraft(prevDraft);
      setError(String(e));
    }
  }, []);

  // 恢复隐藏模型（即时落盘）：从隐藏名单移除后重新出现在列表——价格已删的
  // 以 0 占位，走现有编辑逻辑。按归一移除，同模型多种写法的隐藏条目一并清除，
  // 避免恢复后仍被隐藏。落盘失败报错并回滚本地状态
  const handleRestore = useCallback(async (id: string) => {
    const prev = pricingRef.current;
    if (!prev) return;
    const target = canonicalModelId(id);
    const next: PricingConfig = {
      ...prev,
      hidden: (prev.hidden ?? []).filter((h) => canonicalModelId(h) !== target),
    };
    setPricing(next);
    setError(null);
    try {
      await savePricing(next);
    } catch (e) {
      setPricing(prev);
      setError(String(e));
    }
  }, []);

  // 合并：数据库里的模型 + 价格表里手动加的模型（记忆化，避免每次渲染重算集合与排序）。
  // 必须位于下方 if (!pricing) 早退之前：条件早退会跳过 hooks，导致前后渲染 hooks 数不一致而崩溃
  const modelIds = useMemo(() => {
    if (!pricing) return [];
    // 隐藏名单（已删除模型）：命中归一 key 的不再展示——数据库仍出现过也不显示，
    // 与后端 diff 的遍历主体过滤同口径
    const hiddenSet = new Set((pricing.hidden ?? []).map((h) => canonicalModelId(h)));
    // 按归一化名去重：本地/远端库里仅大小写或隐藏字符不同的写法只保留一行，
    // 首个原始写法作为展示名；另一变体的花费经「小写+点号归一」兜底仍能命中该价格
    const seen = new Set<string>();
    const ids: string[] = [];
    for (const id of [
      ...models.map((m) => m.model_id),
      ...Object.keys(pricing.usd),
    ]) {
      const k = canonicalModelId(id);
      if (hiddenSet.has(k)) continue;
      if (!seen.has(k)) {
        seen.add(k);
        ids.push(id);
      }
    }
    return ids.sort();
  }, [models, pricing]);

  if (!pricing) {
    return <LoadingState text={t("pricing.loading")} />;
  }
  const hiddenList = pricing.hidden ?? [];

  return (
    <PageShell>
      <PageHeader
        title={t("pricing.title")}
        onBack={onBack}
        right={
          <BtnPrimary onClick={handleSave} disabled={saving}>
            {saving ? t("common.saving") : savedFlash ? t("common.saved") : t("common.save")}
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
                  {c === "cny" ? t("pricing.cny") : t("pricing.usd")}
                </PillButton>
              ))}
            </PillGroup>
            <p className="text-[10px] text-slate-500 mt-1.5">
              {t("pricing.unitHint", { rate: fxRate })}
            </p>
            <div className="mt-2 flex items-center justify-between">
              <div className="flex items-center gap-1.5">
                <BtnSecondary onClick={runDiff} className="relative">
                  {t("pricing.checkUpdates")}
                  {/* 红点只对内置表来源展示：models.dev 的差异数在面板内呈现，
                      避免在线同步结果误挂在"检查更新"按钮上 */}
                  {diffSource === "builtin" && updateCount > 0 && (
                    <span className="absolute -top-1 -right-1 min-w-[14px] h-[14px] px-1 flex items-center justify-center rounded-full bg-rose-500 text-white text-[9px] leading-none">
                      {updateCount}
                    </span>
                  )}
                </BtnSecondary>
                <BtnSecondary onClick={runModelsdevSync} disabled={syncing}>
                  {syncing ? t("pricing.syncing") : t("pricing.syncModelsdev")}
                </BtnSecondary>
              </div>
              {diff && updateCount === 0 && !diffPanel && (
                <span className={`text-[10px] ${diff.missing.length > 0 ? "text-amber-600" : "text-emerald-600"}`}>
                  {diff.missing.length > 0
                    ? t("pricing.missingCount", { count: diff.missing.length })
                    : t("pricing.upToDate")}
                </span>
              )}
            </div>
          </>
        }
      />

      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}

        {diffPanel && diff && (
          <SectionCard
            title={
              diffSource === "modelsdev"
                ? t("pricing.diffTitleModelsdev")
                : t("pricing.diffTitle") + (diff.version ? ` v${diff.version}` : "")
            }
          >
            <p className="text-[9px] text-slate-500 mb-2 leading-relaxed">
              {diffSource === "modelsdev"
                ? t("pricing.diffHintModelsdev")
                : t("pricing.diffHint")}
            </p>
            {/* models.dev 数据时间 + 离线缓存标注（在线同步特有） */}
            {diffSource === "modelsdev" && modelsdevMeta && (
              <p className="text-[9px] text-slate-500 -mt-1.5 mb-2">
                {t("pricing.modelsdevFetchedAt", {
                  time: fmtStamp(modelsdevMeta.fetchedAt),
                })}
                {modelsdevMeta.fromCache && (
                  <span className="ml-1 text-amber-600">
                    {t("pricing.modelsdevFromCache")}
                  </span>
                )}
              </p>
            )}
            {/* 无价格模型警示：实际在用但两边都没价格，花费按 0 计 */}
            {diff.missing.length > 0 && (
              <div className="mb-2 rounded-md bg-amber-500/10 border border-amber-500/20 px-2 py-1.5">
                <div className="text-[9px] text-amber-700/90 font-medium">
                  {t("pricing.missingWarn", { count: diff.missing.length })}
                </div>
                <ul className="mt-0.5 max-h-16 overflow-y-auto space-y-0.5">
                  {diff.missing.map((m) => (
                    <li key={m} className="text-[9px] text-slate-700/60 num break-words">
                      {m}
                    </li>
                  ))}
                </ul>
                <div className="text-[8px] text-slate-700/40 mt-0.5">
                  {t("pricing.addPriceBelow")}
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
                          {t("pricing.badgeNew")}
                        </span>
                        <span className="font-medium text-slate-900/85 text-[10px] break-words">
                          {i.model_id}
                        </span>
                        {i.reference_id && (
                          <span
                            title={t("pricing.refFrom", { model: i.reference_id })}
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
                          {t("pricing.badgeChanged")}
                        </span>
                        <span className="font-medium text-slate-900/85 text-[10px] break-words">
                          {i.model_id}
                        </span>
                      </div>
                      <div className="mt-0.5 text-[10px] num leading-relaxed">
                        <span className="text-slate-700/40 text-[9px]">
                          {t("pricing.old")}{" "}
                        </span>
                        <span className="text-slate-700/55 whitespace-nowrap">
                          {u ? `$ ${fmtTriplet(u)}` : t("pricing.unpriced")}
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
                  {t("pricing.noDiff")}
                </div>
              )}
            </div>
            <div className="flex items-center justify-between mt-2">
              <BtnSecondary onClick={() => setDiffPanel(false)}>
                {t("pricing.collapse")}
              </BtnSecondary>
              <BtnPrimary onClick={handleApplyUpdates} disabled={applying || selected.size === 0}>
                {applying
                  ? t("pricing.applying")
                  : selected.size > 0
                    ? t("pricing.applySelectedCount", { count: selected.size })
                    : t("pricing.applySelected")}
              </BtnPrimary>
            </div>
          </SectionCard>
        )}

        {modelIds.map((id) => (
          <ModelPriceRow
            key={id}
            id={id}
            price={lookupPrice(pricing.usd, id) ?? ZERO_PRICE}
            dIn={draft[`${id}|input`]}
            dOut={draft[`${id}|output`]}
            dCache={draft[`${id}|cache_read`]}
            onDraftChange={handleDraftChange}
            onCommit={commitDraft}
            onRemove={handleRemove}
          />
        ))}
        {modelIds.length === 0 && (
          <div className="text-center text-xs text-slate-500 py-8">
            {t("pricing.noModels")}
          </div>
        )}

        {/* 已隐藏模型（删除过的）：折叠入口 + 恢复面板（N=0 时不渲染） */}
        {hiddenList.length > 0 && (
          <div className="card-base rounded-2xl p-3">
            <button
              type="button"
              onClick={() => setHiddenOpen((o) => !o)}
              className="text-[10px] text-slate-700/55 hover:text-slate-900/80 transition-colors"
            >
              {hiddenOpen ? "▾" : "▸"}{" "}
              {t("pricing.hiddenCount", { count: hiddenList.length })}
            </button>
            {hiddenOpen && (
              <ul className="mt-2 space-y-1">
                {hiddenList.map((h) => (
                  <li
                    key={h}
                    className="flex items-center justify-between gap-2"
                  >
                    <span className="min-w-0 break-all num text-[10px] text-slate-900/80">
                      {h}
                    </span>
                    <button
                      type="button"
                      onClick={() => handleRestore(h)}
                      className="shrink-0 px-1.5 py-0.5 rounded text-[10px] text-sky-600/90 hover:bg-sky-500/10 transition-colors"
                    >
                      {t("pricing.restore")}
                    </button>
                  </li>
                ))}
              </ul>
            )}
          </div>
        )}
      </PageBody>
    </PageShell>
  );
}

/** 单个模型的价格卡片（memo：草稿用三个原始类型 props 传递，
 *  键入时只重渲染这一张卡片而非整个模型列表）。表单恒定为美元编辑。
 *  右上角删除按钮（hover 显示）：从列表移除该模型，即时落盘 */
const ModelPriceRow = memo(function ModelPriceRow({
  id,
  price,
  dIn,
  dOut,
  dCache,
  onDraftChange,
  onCommit,
  onRemove,
}: {
  id: string;
  price: ModelPrice;
  dIn: string | undefined;
  dOut: string | undefined;
  dCache: string | undefined;
  onDraftChange: (id: string, key: keyof ModelPrice, v: string) => void;
  onCommit: (id: string, key: keyof ModelPrice) => void;
  onRemove: (id: string) => void;
}) {
  const { t } = useI18n();
  const drafts: Record<keyof ModelPrice, string | undefined> = {
    input: dIn,
    output: dOut,
    cache_read: dCache,
  };
  return (
    <div className="card-base rounded-2xl p-3 group">
      <div className="flex items-start justify-between gap-2 mb-2">
        <div className="min-w-0 break-all text-xs font-medium text-slate-900/90">
          {id}
        </div>
        <button
          type="button"
          title={t("pricing.removeModel")}
          aria-label={t("pricing.removeModel")}
          onClick={() => onRemove(id)}
          className="shrink-0 px-1.5 rounded text-[10px] leading-5 text-slate-700/40 hover:text-red-600 hover:bg-red-500/10 opacity-0 group-hover:opacity-100 focus-visible:opacity-100 transition-opacity"
        >
          ×
        </button>
      </div>
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
              <span className="text-slate-700/55">{t(f.labelKey)}</span>
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

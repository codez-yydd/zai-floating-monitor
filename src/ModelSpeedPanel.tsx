import { useCallback, useEffect, useMemo, useState } from "react";
import type { ModelSpeedStat } from "./types";
import { getModelSpeed } from "./api";
import { formatSeconds, formatTokens, formatTps } from "./format";
import { useI18n } from "./i18n";

/** 时间范围（今日 = 自然本地日；后端 days=0 使用同一口径） */
type SpeedRange = "today" | "7d";

const SPEED_RANGE_KEY = "zbar-speed-range";

function loadSpeedRange(): SpeedRange {
  try {
    const saved = localStorage.getItem(SPEED_RANGE_KEY);
    if (saved === "today" || saved === "7d") return saved;
  } catch {
    // 偏好只是锦上添花，存储不可用时使用默认范围。
  }
  return "7d";
}

function sortByP50(rows: ModelSpeedStat[]): ModelSpeedStat[] {
  return [...rows].sort((a, b) => {
    if (a.p50Tps == null && b.p50Tps == null) {
      return b.requests - a.requests || a.modelId.localeCompare(b.modelId);
    }
    if (a.p50Tps == null) return 1;
    if (b.p50Tps == null) return -1;
    return b.p50Tps - a.p50Tps || b.requests - a.requests;
  });
}

function tps(value: number | null): string {
  return value == null ? "—" : formatTps(value);
}

function seconds(value: number | null): string {
  return value == null ? "—" : formatSeconds(value);
}

function percent(value: number | null): string {
  return value == null ? "—" : `${Math.round(value * 100)}%`;
}

function speedBarColor(value: number | null): string {
  if (value == null) return "bg-slate-400/60";
  if (value >= 80) return "bg-emerald-500";
  if (value >= 40) return "bg-amber-500";
  return "bg-rose-500";
}

function SpeedGlyph() {
  return (
    <span className="speed-glyph" aria-hidden="true">
      <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.8">
        <path strokeLinecap="round" strokeLinejoin="round" d="M4 17.5 9.2 12l3.2 3.1L20 7.5" />
        <path strokeLinecap="round" d="M15.5 7.5H20v4.5" />
      </svg>
    </span>
  );
}

function RefreshGlyph() {
  return (
    <svg viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" aria-hidden="true">
      <path strokeLinecap="round" strokeLinejoin="round" d="M20 11a8 8 0 0 0-14.7-3.9L4 9" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 5v4h4" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M4 13a8 8 0 0 0 14.7 3.9L20 15" />
      <path strokeLinecap="round" strokeLinejoin="round" d="M20 19v-4h-4" />
    </svg>
  );
}

function OverviewMetric({ label, value }: { label: string; value: string }) {
  return (
    <div className="speed-overview-metric">
      <div className="speed-overview-label">{label}</div>
      <div className="speed-overview-value num">{value}</div>
    </div>
  );
}

function MiniMetric({
  label,
  value,
  primary = false,
  unit,
}: {
  label: string;
  value: string;
  primary?: boolean;
  unit?: string;
}) {
  return (
    <div className={`speed-mini-metric ${primary ? "speed-mini-primary" : ""}`}>
      <div className="speed-mini-label">{label}</div>
      <div className="speed-mini-value-row">
        <div className="speed-mini-value num">{value}</div>
        {unit && <span className="speed-mini-unit">{unit}</span>}
      </div>
    </div>
  );
}

function DetailChip({ label, value, warn = false }: { label: string; value: string; warn?: boolean }) {
  return (
    <div className={`speed-detail-chip ${warn ? "speed-detail-warn" : ""}`}>
      <span className="speed-detail-label">{label}</span>
      <span className="speed-detail-value num">{value}</span>
    </div>
  );
}

function ModelSpeedRow({
  model,
  rank,
  maxP50,
}: {
  model: ModelSpeedStat;
  rank: number;
  maxP50: number;
}) {
  const { t } = useI18n();
  const primarySpeed = model.p50Tps ?? model.avgTps;
  const ratio = model.p50Tps != null && maxP50 > 0 ? model.p50Tps / maxP50 : 0;
  const lowSuccess = model.successRate != null && model.successRate < 0.95;
  const sampleText = t("speed.samplesCount", { n: formatTokens(model.requests) });
  const provider = model.provider || t("speed.providerUnknown");

  return (
    <article className={`speed-model-card ${rank === 0 ? "speed-model-featured" : ""}`}>
      <div className="flex items-start gap-2.5 min-w-0">
        <span className={`speed-rank ${rank === 0 ? "speed-rank-first" : ""}`}>{rank + 1}</span>
        <div className="min-w-0 flex-1 pt-px">
          <div className="flex items-center gap-1.5 min-w-0">
            <span className="speed-model-name truncate">{model.modelId}</span>
            {lowSuccess && <span className="speed-warning-dot" />}
          </div>
          <div className="flex items-center gap-1.5 mt-1 min-w-0">
            <span className="speed-provider truncate">{provider}</span>
            <span className="speed-sample-count shrink-0">{sampleText}</span>
          </div>
        </div>
        <div className="shrink-0 text-right">
          <div className="flex items-baseline justify-end gap-0.5 whitespace-nowrap">
            <span className="speed-primary-value num">{tps(primarySpeed)}</span>
            <span className="speed-primary-unit">t/s</span>
          </div>
          <div className="speed-primary-label">{t("speed.p50")}</div>
        </div>
      </div>

      <div className="speed-bar-wrap" aria-label={t("speed.relativeSpeed", { pct: Math.round(ratio * 100) })}>
        <div className="speed-bar-meta">
          <span>{t("speed.relativeLabel")}</span>
          <span className="speed-bar-label num">{Math.round(ratio * 100)}%</span>
        </div>
        <div className="speed-bar-track">
          <div className={`speed-bar-fill ${speedBarColor(model.p50Tps)}`} style={{ width: `${Math.min(Math.max(ratio * 100, 0), 100)}%` }} />
        </div>
      </div>

      <div className="grid grid-cols-4 gap-1.5 mt-2.5">
        <MiniMetric label={t("speed.p10")} value={tps(model.p10Tps)} unit="t/s" />
        <MiniMetric label={t("speed.p50")} value={tps(model.p50Tps)} primary unit="t/s" />
        <MiniMetric label={t("speed.p90")} value={tps(model.p90Tps)} unit="t/s" />
        <MiniMetric label={t("speed.ttft")} value={seconds(model.ttftAvgMs)} />
      </div>

      <div className="grid grid-cols-2 gap-1.5 mt-1.5">
        <DetailChip
          label={t("speed.inputShort")}
          value={t("speed.averagePeak", {
            avg: formatTokens(model.inAvg),
            max: formatTokens(model.inMax),
          })}
        />
        <DetailChip
          label={t("speed.outputShort")}
          value={t("speed.averagePeak", {
            avg: formatTokens(model.outAvg),
            max: formatTokens(model.outMax),
          })}
        />
        <DetailChip
          label={t("speed.latencyShort")}
          value={t("speed.averageSlower", {
            avg: seconds(model.durAvgMs),
            slow: seconds(model.durP90Ms),
          })}
        />
        <DetailChip
          label={t("speed.samplesShort")}
          value={t("speed.requestSummary", {
            n: String(model.requests),
            pct: percent(model.successRate),
          })}
          warn={lowSuccess}
        />
      </div>
    </article>
  );
}

function SpeedRangeToggle({ range, onChange }: { range: SpeedRange; onChange: (range: SpeedRange) => void }) {
  const { t } = useI18n();
  return (
    <div className="speed-range" role="tablist" aria-label={t("speed.rangeLabel")}>
      {(["today", "7d"] as const).map((item) => (
        <button
          key={item}
          type="button"
          role="tab"
          aria-selected={range === item}
          className={`speed-range-button ${range === item ? "speed-range-active" : ""}`}
          onClick={() => onChange(item)}
        >
          {item === "today" ? t("speed.rangeToday") : t("speed.range7d")}
        </button>
      ))}
    </div>
  );
}

export function ModelSpeedPanel({ refreshToken = 0 }: { refreshToken?: number }) {
  const { t } = useI18n();
  const [range, setRange] = useState<SpeedRange>(loadSpeedRange);
  const [stats, setStats] = useState<ModelSpeedStat[] | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [updatedAt, setUpdatedAt] = useState<number | null>(null);
  const [now, setNow] = useState(() => Date.now());
  const [loading, setLoading] = useState(false);

  useEffect(() => {
    try {
      localStorage.setItem(SPEED_RANGE_KEY, range);
    } catch {
      // ignore storage errors
    }
  }, [range]);

  const load = useCallback(
    (resetError: boolean) => {
      if (resetError) setError(null);
      setLoading(true);
      getModelSpeed(range === "today" ? 0 : 7)
        .then((rows) => {
          setStats(rows);
          setUpdatedAt(Date.now());
          setError(null);
        })
        .catch((e) => setError(String(e)))
        .finally(() => setLoading(false));
    },
    [range]
  );

  useEffect(() => {
    let cancelled = false;
    const refresh = (resetError: boolean) => {
      if (!cancelled) load(resetError);
    };
    refresh(true);
    const timer = window.setInterval(() => refresh(false), 30_000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [load, refreshToken]);

  useEffect(() => {
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => window.clearInterval(timer);
  }, []);

  const rows = useMemo(() => (stats ? sortByP50(stats) : []), [stats]);
  const maxP50 = rows.reduce((max, row) => Math.max(max, row.p50Tps ?? 0), 0);
  const overview = useMemo(() => {
    const requests = rows.reduce((sum, row) => sum + row.requests, 0);
    const speedRows = rows.filter((row) => row.p50Tps != null);
    const meanP50 = speedRows.length > 0
      ? speedRows.reduce((sum, row) => sum + (row.p50Tps ?? 0), 0) / speedRows.length
      : null;
    const successRows = rows.filter((row) => row.successRate != null);
    const successRequests = successRows.reduce((sum, row) => sum + row.requests, 0);
    const success = successRequests > 0
      ? successRows.reduce((sum, row) => sum + row.requests * (row.successRate ?? 0), 0) / successRequests
      : null;
    return { requests, meanP50, success };
  }, [rows]);

  const fastest = rows.find((row) => row.p50Tps != null)?.p50Tps ?? null;
  const stamp = updatedAt == null ? "" : new Date(updatedAt).toTimeString().slice(0, 8);
  const ago = updatedAt == null ? 0 : Math.max(0, Math.round((now - updatedAt) / 1000));

  const content = stats === null ? (
    error ? (
      <div className="speed-state speed-state-error">
        <div className="speed-state-icon">!</div>
        <div className="text-xs text-slate-700/80 font-medium">{t("speed.loadFailed")}</div>
        <div className="text-[10px] text-red-700/80 break-all max-w-[230px] text-center">{error}</div>
        <button type="button" className="speed-retry-button" onClick={() => load(true)}>{t("speed.retry")}</button>
      </div>
    ) : (
      <div className="speed-state">
        <div className="speed-loading-orb" />
        <div className="text-xs text-slate-600/80">{t("common.loading")}</div>
      </div>
    )
  ) : rows.length === 0 ? (
    <div className="speed-state speed-state-empty">
      <div className="speed-empty-chart" aria-hidden="true"><span /><span /><span /><span /></div>
      <div className="text-xs text-slate-700/80 font-medium">{t("speed.empty")}</div>
      <div className="text-[10px] text-slate-500 leading-relaxed max-w-[220px] text-center">{t("speed.emptyHint")}</div>
      <button type="button" className="speed-retry-button" onClick={() => load(true)}>{t("speed.refresh")}</button>
    </div>
  ) : (
    <>
      <div className="speed-list-heading">
        <div className="flex items-center gap-1.5">
          <span className="speed-heading-dot" />
          <span>{t("speed.ranking")}</span>
        </div>
        <span className="num">{t("speed.modelCount", { n: rows.length })}</span>
      </div>
      <div className="flex flex-col gap-2">
        {rows.map((row, index) => <ModelSpeedRow key={row.modelId} model={row} rank={index} maxP50={maxP50} />)}
      </div>
    </>
  );

  return (
    <div className="speed-page flex-1 min-h-0 overflow-y-auto px-3 py-3">
      <section className="speed-hero">
        <div className="flex items-start justify-between gap-2">
          <div className="flex items-center gap-2 min-w-0">
            <SpeedGlyph />
            <div className="min-w-0">
              <h2 className="speed-hero-title truncate">{t("speed.title")}</h2>
              <p className="speed-hero-subtitle truncate">{t("speed.subtitle")}</p>
            </div>
          </div>
          <div className="flex items-center gap-1.5 shrink-0">
            <SpeedRangeToggle range={range} onChange={setRange} />
            <button type="button" className={`speed-refresh ${loading ? "speed-refresh-loading" : ""}`} onClick={() => load(true)} aria-label={t("speed.refresh")}>
              <RefreshGlyph />
            </button>
          </div>
        </div>

        <div className="grid grid-cols-[1.08fr_1fr] gap-2 mt-3">
          <div className="speed-fastest-block">
            <div className="speed-kicker">{t("speed.fastest")}</div>
            <div className="flex items-baseline gap-1 mt-1">
              <span className="speed-fastest-value num">{tps(fastest)}</span>
              <span className="speed-fastest-unit">t/s</span>
            </div>
            <div className="speed-fastest-caption">{t("speed.fastestHint")}</div>
          </div>
          <div className="grid grid-cols-2 gap-1.5">
            <OverviewMetric label={t("speed.overviewModels")} value={String(rows.length)} />
            <OverviewMetric label={t("speed.overviewRequests")} value={formatTokens(overview.requests)} />
            <OverviewMetric label={t("speed.overviewMean")} value={overview.meanP50 == null ? "—" : `${formatTps(overview.meanP50)}`} />
            <OverviewMetric label={t("speed.overviewSuccess")} value={percent(overview.success)} />
          </div>
        </div>

        <div className="speed-hero-footer">
          <span>{updatedAt == null ? t("speed.waitingData") : t("speed.updatedAt", { time: stamp, ago: String(ago) })}</span>
          <span className="speed-live-mark"><i />{t("speed.autoRefresh")}</span>
        </div>
      </section>

      {error && stats !== null && <div className="speed-inline-error">{t("speed.fail", { msg: error })}</div>}
      <section className="speed-results">{content}</section>
    </div>
  );
}

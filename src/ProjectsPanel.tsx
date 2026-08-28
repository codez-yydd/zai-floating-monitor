import { useEffect, useRef, useState } from "react";
import type {
  Currency,
  ProjectSummary,
  RangePreset,
  SessionSummary,
} from "./types";
import { getProjectSessions, getProjects } from "./api";
import { formatCost, formatMs, formatResetStamp, formatTokens, formatTps } from "./format";
import { sourceMeta } from "./agentVisibility";
import { resolveRange } from "./RangePicker";
import { useI18n } from "./i18n";
import { AlertBanner, BtnSecondary, EmptyState, LoadingState } from "./layout";

interface Props {
  /** 与 StatsPanel 顶栏共享的时间范围状态（沿用现有范围传递方式） */
  preset: RangePreset;
  custom: { from: string; to: string };
  currency: Currency;
  fxRate: number;
}

/** 会话分页大小（后端 limit 参数口径） */
const PAGE_SIZE = 50;

/** 项目行花费：主行当前币种（人民币按汇率折算，与汇总页同款），美元作小字附注 */
function CostCell({
  usd,
  currency,
  fxRate,
  size = "sm",
}: {
  usd: number;
  currency: Currency;
  fxRate: number;
  size?: "sm" | "xs";
}) {
  const main = formatCost(currency === "cny" ? usd * fxRate : usd, currency);
  return (
    <span className="text-right shrink-0">
      <span
        className={`num font-semibold text-slate-900/85 block ${
          size === "sm" ? "text-[11px]" : "text-[10px]"
        }`}
      >
        {main}
      </span>
      {currency === "cny" && usd !== 0 && (
        <span className="num text-[9px] text-slate-700/45 block">
          ${usd < 0.01 && usd > 0 ? usd.toFixed(4) : usd.toFixed(2)}
        </span>
      )}
    </span>
  );
}

/** Agent 来源小徽标（品牌色浅底 + 品牌名） */
function SourceBadge({ source }: { source: string }) {
  const meta = sourceMeta(source);
  return (
    <span
      className="shrink-0 px-1 py-px rounded text-[8px] font-medium whitespace-nowrap"
      style={{ background: `${meta.color}1f`, color: meta.color }}
    >
      {meta.label}
    </span>
  );
}

/** 会话起止时间：同日「MM-DD HH:mm ~ HH:mm」，跨日两端完整「MM-DD HH:mm」 */
function sessionTimeText(first: number, last: number): string {
  const a = formatResetStamp(first);
  const b = formatResetStamp(last);
  return a.slice(0, 5) === b.slice(0, 5) ? `${a} ~ ${b.slice(6)}` : `${a} ~ ${b}`;
}

/** 项目列表页的一行：路径 + 花费 + 指标 + Agent 构成堆叠条与图例 */
function ProjectRow({
  project,
  currency,
  fxRate,
  onOpen,
}: {
  project: ProjectSummary;
  currency: Currency;
  fxRate: number;
  onOpen: () => void;
}) {
  const { t } = useI18n();
  const agents = [...project.by_agent]
    .filter((a) => a.tokens > 0)
    .sort((a, b) => b.tokens - a.tokens);
  const agentTokens = agents.reduce((s, a) => s + a.tokens, 0);

  // 堆叠条：gradient 硬切色段（与汇总页 CompositionBar 同款做法，避免细缝）
  let acc = 0;
  const stops: string[] = [];
  for (const a of agents) {
    const meta = sourceMeta(a.source);
    const start = agentTokens > 0 ? (acc / agentTokens) * 100 : 0;
    acc += a.tokens;
    const end = agentTokens > 0 ? (acc / agentTokens) * 100 : 0;
    stops.push(`${meta.color} ${start}%`, `${meta.color} ${end}%`);
  }

  return (
    <button
      onClick={onOpen}
      className="card-base rounded-xl px-2.5 py-2 w-full text-left hover:bg-slate-900/5 transition-colors"
    >
      <div className="flex items-start gap-2">
        <span
          className={`flex-1 min-w-0 text-[11px] font-medium text-slate-900/85 truncate ${
            project.is_unknown ? "text-slate-700/60" : ""
          }`}
          title={project.display_path ?? undefined}
        >
          {project.is_unknown ? t("projects.unknown") : project.display_path}
        </span>
        <CostCell usd={project.cost_usd} currency={currency} fxRate={fxRate} />
      </div>
      <div className="flex items-center gap-2 num text-[9px] text-slate-700/45 mt-0.5">
        <span title={t("common.totalTokens")}>{formatTokens(project.total_tokens)} Token</span>
        <span title={t("common.requests")}>
          {t("common.requests")} {formatTokens(project.requests)}
        </span>
        <span title={t("projects.sessionCount", { count: project.sessions })}>
          {t("projects.sessionCount", { count: project.sessions })}
        </span>
      </div>
      {agents.length > 0 && agentTokens > 0 && (
        <>
          <div
            className="h-1.5 rounded-full mt-1.5"
            title={t("projects.agentMix")}
            style={{
              background: `linear-gradient(90deg, ${stops.join(", ")})`,
            }}
          />
          <div className="flex flex-wrap gap-x-2 gap-y-0.5 mt-1">
            {agents.map((a) => {
              const meta = sourceMeta(a.source);
              return (
                <span
                  key={a.source}
                  className="flex items-center gap-1 text-[8px] text-slate-700/50"
                >
                  <span
                    className="w-1.5 h-1.5 rounded-full shrink-0"
                    style={{ background: meta.color }}
                  />
                  {meta.label}
                  <span className="num">{formatTokens(a.tokens)}</span>
                </span>
              );
            })}
          </div>
        </>
      )}
    </button>
  );
}

/** 会话列表页的一行：时间 + Agent 徽标 + 模型标签 + Token 明细 + 花费/时长 */
function SessionRow({
  session,
  currency,
  fxRate,
}: {
  session: SessionSummary;
  currency: Currency;
  fxRate: number;
}) {
  const { t } = useI18n();
  const shownModels = session.models.slice(0, 3);
  const hiddenModels = session.models.length - shownModels.length;
  return (
    <div className="card-base rounded-xl px-2.5 py-2">
      <div className="flex items-center gap-1.5 min-w-0">
        <span
          className="num text-[9px] text-slate-700/55 whitespace-nowrap shrink-0"
          title={session.session_id}
        >
          {sessionTimeText(session.first_at, session.last_at)}
        </span>
        <SourceBadge source={session.source} />
        <span className="flex-1 min-w-0 flex gap-1 overflow-hidden justify-end">
          {shownModels.map((m) => (
            <span
              key={m}
              className="rounded bg-slate-900/6 px-1 py-px text-[8px] text-slate-700/60 truncate max-w-[88px] whitespace-nowrap"
              title={`${t("projects.models")}: ${m}`}
            >
              {m}
            </span>
          ))}
          {hiddenModels > 0 && (
            <span className="text-[8px] text-slate-700/40 shrink-0">
              +{hiddenModels}
            </span>
          )}
        </span>
      </div>
      <div className="flex items-end justify-between gap-2 mt-1 min-w-0">
        <div className="flex flex-wrap gap-x-2 num text-[9px] text-slate-700/55 min-w-0">
          <span title={t("projects.in")}>
            {t("projects.in")} {formatTokens(session.input_tokens)}
          </span>
          <span title={t("projects.out")}>
            {t("projects.out")} {formatTokens(session.output_tokens)}
          </span>
          {session.cache_read_tokens > 0 && (
            <span title={t("projects.cacheRead")}>
              {t("projects.cacheRead")} {formatTokens(session.cache_read_tokens)}
            </span>
          )}
          {session.cache_write_tokens > 0 && (
            <span title={t("projects.cacheWrite")}>
              {t("projects.cacheWrite")}{" "}
              {formatTokens(session.cache_write_tokens)}
            </span>
          )}
          <span title={t("common.requests")}>
            {t("common.requests")} {formatTokens(session.requests)}
          </span>
          {session.speed_tps != null && (
            <span title={t("common.avgSpeed")}>
              {t("projects.speed")} {formatTps(session.speed_tps)} t/s
            </span>
          )}
          {session.ttft_ms != null && (
            <span title={t("common.ttft")}>
              {t("projects.ttft")} {formatMs(session.ttft_ms)}
            </span>
          )}
        </div>
        <div className="flex items-center gap-2 shrink-0">
          {session.wall_duration_ms > 0 && (
            <span
              className="num text-[9px] text-slate-700/45"
              title={t("projects.duration")}
            >
              {formatMs(session.wall_duration_ms)}
            </span>
          )}
          <CostCell
            usd={session.cost_usd}
            currency={currency}
            fxRate={fxRate}
            size="xs"
          />
        </div>
      </div>
    </div>
  );
}

/**
 * 项目 / 会话浏览器（统计面板「项目」标签页）。
 *
 * 数据不进 DataCache 30s 轮询：进入标签页（组件挂载）或范围变化时按需拉取
 * get_projects；下钻会话列表分页拉 get_project_sessions。
 */
export function ProjectsPanel({ preset, custom, currency, fxRate }: Props) {
  const { t } = useI18n();
  const [fromMs, toMs] = resolveRange(preset, custom);

  // ===== 项目列表 =====
  const [projects, setProjects] = useState<ProjectSummary[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const projectsReqId = useRef(0);

  const loadProjects = () => {
    const reqId = ++projectsReqId.current;
    setLoading(true);
    setError(null);
    getProjects(fromMs, toMs)
      .then((list) => {
        if (reqId !== projectsReqId.current) return;
        // 已知项目按花费降序，未知项目置底
        const sorted = [...list].sort(
          (a, b) =>
            Number(a.is_unknown) - Number(b.is_unknown) ||
            b.cost_usd - a.cost_usd
        );
        setProjects(sorted);
      })
      .catch((e) => {
        if (reqId !== projectsReqId.current) return;
        setError(String(e));
      })
      .finally(() => {
        if (reqId === projectsReqId.current) setLoading(false);
      });
  };

  // resolveRange 的 toMs 含 Date.now()，每次渲染都是新值，不可作为 effect 依赖；
  // 改用稳定的 preset/custom，loadProjects 闭包内的 fromMs/toMs 在触发时即最新值。
  useEffect(() => {
    loadProjects();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [preset, custom]);

  // ===== 会话下钻 =====
  const [sel, setSel] = useState<ProjectSummary | null>(null);
  const [sourceFilter, setSourceFilter] = useState<string | null>(null);
  const [sessions, setSessions] = useState<SessionSummary[]>([]);
  const [sessionTotal, setSessionTotal] = useState(0);
  const [sessionsLoading, setSessionsLoading] = useState(false);
  const [loadingMore, setLoadingMore] = useState(false);
  const [sessionsError, setSessionsError] = useState<string | null>(null);
  const sessionsReqId = useRef(0);

  const openProject = (p: ProjectSummary) => {
    setSel(p);
    setSourceFilter(null);
    setSessions([]);
    setSessionTotal(0);
    setSessionsError(null);
  };

  const backToList = () => {
    setSel(null);
    setSessions([]);
    setSessionTotal(0);
    setSessionsError(null);
  };

  /** 拉取一页会话；append=false 时重置列表（筛选/翻首页） */
  const loadSessions = (offset: number, append: boolean) => {
    if (!sel) return;
    const reqId = ++sessionsReqId.current;
    if (append) setLoadingMore(true);
    else setSessionsLoading(true);
    setSessionsError(null);
    getProjectSessions(sel.key, fromMs, toMs, sourceFilter, offset, PAGE_SIZE)
      .then((page) => {
        if (reqId !== sessionsReqId.current) return;
        setSessionTotal(page.total);
        setSessions((prev) => (append ? [...prev, ...page.items] : page.items));
      })
      .catch((e) => {
        if (reqId !== sessionsReqId.current) return;
        setSessionsError(String(e));
      })
      .finally(() => {
        if (reqId !== sessionsReqId.current) return;
        setSessionsLoading(false);
        setLoadingMore(false);
      });
  };

  // 下钻页/筛选/范围变化 → 重新拉第一页
  useEffect(() => {
    if (sel) loadSessions(0, false);
    // sel 变化由 openProject 触发；此处响应 sel/sourceFilter/范围
    // resolveRange 的 toMs 含 Date.now()，不可作为 effect 依赖，改用稳定的 preset/custom
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [sel, sourceFilter, preset, custom]);

  const hasMore = sel != null && sessions.length < sessionTotal;
  // 该项目出现过的 Agent 来源（按 tokens 降序），作为筛选下拉选项
  const filterOptions = sel
    ? [...sel.by_agent]
        .filter((a) => a.sessions > 0)
        .sort((a, b) => b.tokens - a.tokens)
        .map((a) => a.source)
    : [];
  const selName = sel
    ? sel.is_unknown
      ? t("projects.unknown")
      : sel.display_path ?? sel.key
    : "";

  return (
    <div className="flex flex-col h-full min-h-0">
      {/* 下钻页工具行：面包屑返回 + Agent 筛选（列表页无工具行，范围用顶栏共享 RangePicker） */}
      {sel && (
        <div className="px-3 pt-2 pb-1.5 border-b border-slate-900/8 shrink-0 flex items-center gap-1.5">
          <button
            onClick={backToList}
            className="btn-ghost text-[11px] px-1 -ml-1 shrink-0"
          >
            {t("projects.back")}
          </button>
          <span
            className="flex-1 min-w-0 text-[11px] font-medium text-slate-900/85 truncate"
            title={selName}
          >
            {t("projects.sessionsOf", { name: selName })}
          </span>
          <select
            value={sourceFilter ?? ""}
            onChange={(e) => setSourceFilter(e.target.value || null)}
            title={t("projects.agentFilter")}
            className="num w-[4.75rem] shrink-0 px-1 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60"
          >
            <option value="">{t("projects.all")}</option>
            {filterOptions.map((s) => (
              <option key={s} value={s}>
                {sourceMeta(s).label}
              </option>
            ))}
          </select>
        </div>
      )}

      <div
        className={`flex-1 min-h-0 overflow-y-auto px-3 py-2.5 page-stack ${
          sel ? "pt-2.5" : ""
        }`}
      >
        {/* ===== 项目列表页 ===== */}
        {!sel && (error || loading || projects === null || projects.length === 0) && (
          <>
            {error && <AlertBanner>{t("projects.loadFail", { msg: error })}</AlertBanner>}
            {loading || projects === null ? (
              <LoadingState />
            ) : (
              !error && (
                <EmptyState
                  title={t("projects.empty")}
                  hint={t("projects.emptyHint")}
                />
              )
            )}
          </>
        )}

        {!sel && !loading && projects !== null && projects.length > 0 && (
          <>
            {projects.map((p) => (
              <ProjectRow
                key={p.key}
                project={p}
                currency={currency}
                fxRate={fxRate}
                onOpen={() => openProject(p)}
              />
            ))}
            <div className="flex justify-end">
              <BtnSecondary onClick={loadProjects} className="text-[10px]!">
                {t("common.refresh")}
              </BtnSecondary>
            </div>
          </>
        )}

        {/* ===== 会话列表页 ===== */}
        {sel && (
          <>
            {sessionsError && (
              <AlertBanner>
                {t("projects.sessionsLoadFail", { msg: sessionsError })}
              </AlertBanner>
            )}
            {sessionsLoading ? (
              <LoadingState />
            ) : sessions.length === 0 && !sessionsError ? (
              <EmptyState title={t("projects.sessionsEmpty")} />
            ) : (
              <>
                {sessions.map((s) => (
                  <SessionRow
                    key={s.session_id + s.first_at}
                    session={s}
                    currency={currency}
                    fxRate={fxRate}
                  />
                ))}
                {hasMore && (
                  <div className="flex justify-center">
                    <BtnSecondary
                      onClick={() => loadSessions(sessions.length, true)}
                      disabled={loadingMore}
                      className="text-[10px]!"
                    >
                      {loadingMore
                        ? t("projects.loadingMore")
                        : t("projects.loadMore", {
                            loaded: sessions.length,
                            total: sessionTotal,
                          })}
                    </BtnSecondary>
                  </div>
                )}
                {!hasMore && sessions.length > 0 && (
                  <div className="text-center text-[9px] text-slate-700/40">
                    {t("projects.noMore")}
                  </div>
                )}
              </>
            )}
          </>
        )}
      </div>
    </div>
  );
}

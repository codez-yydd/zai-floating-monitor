import { useEffect, useMemo, useRef, useState } from "react";
import type {
  PricingConfig,
  Stats,
  TrendBucket,
  TrendPoint,
} from "./types";
import {
  fetchClaudeUsage,
  fetchCodexUsage,
  fetchCursorUsage,
  fetchKimiUsage,
  fetchStats,
  fetchTrend,
  getCursorConfig,
  getProjects,
  saveShareImage,
} from "./api";
import { formatCost, formatTokens } from "./format";
import { modelCost } from "./merge";
import { foldCursorModelRows, foldModelStatRows } from "./modelName";
import { sourceMeta, type AgentVisibility } from "./agentVisibility";
import { BtnPrimary, BtnSecondary } from "./layout";
import { useI18n } from "./i18n";

// ============================================================
// 花费分享卡片：纯 Canvas 2D 手绘（零第三方依赖）。
// 毛玻璃面板无法经 foreignObject 截图，故用实色深底渐变在 canvas 上
// 模拟深色卡片风格；固定 2x 缩放导出保证清晰度。
// ============================================================

const DAY_MS = 86_400_000;

/** 画布逻辑宽度（实际导出为 ×2 物理像素） */
const CARD_W = 1080;
const CARD_PAD = 64;
/** 导出缩放：固定 2x，与设备 devicePixelRatio 无关，跨设备一致清晰 */
const CARD_SCALE = 2;

const DARK_BG_TOP = "#0c1526";
const DARK_BG_MID = "#101f36";
const DARK_BG_BOTTOM = "#0a1220";
const TEXT_PRIMARY = "#f1f5f9";
const TEXT_SECONDARY = "#94a3b8";
const TEXT_FAINT = "#64748b";
const ACCENT = "#38bdf8";
const BAR_TRACK = "rgba(148, 163, 184, 0.14)";

const FONT_STACK =
  "-apple-system, BlinkMacSystemFont, 'PingFang SC', 'Hiragino Sans GB', 'Microsoft YaHei', 'Segoe UI', sans-serif";

function startOfLocalDay(ms: number): number {
  const d = new Date(ms);
  d.setHours(0, 0, 0, 0);
  return d.getTime();
}

/** 本地日期 MM-DD（与后端 day 趋势桶标签同格式） */
function mdLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${p(d.getMonth() + 1)}-${p(d.getDate())}`;
}

function ymdLabel(ms: number): string {
  const d = new Date(ms);
  const p = (n: number) => String(n).padStart(2, "0");
  return `${d.getFullYear()}${p(d.getMonth() + 1)}${p(d.getDate())}`;
}

// ===== 卡片数据结构 =====

interface ShareModelRow {
  name: string;
  color: string;
  tokens: number;
  costUsd: number;
}

interface ShareProjectRow {
  name: string;
  tokens: number;
  costUsd: number;
}

interface ShareData {
  /** 近 7 天范围（含今天） */
  fromMs: number;
  toMs: number;
  /** USD→CNY 汇率（总花费人民币展示折算用，取自 Cursor 配置） */
  fxRate: number;
  totalCostUsd: number;
  tokens: number;
  requests: number;
  models: ShareModelRow[];
  projects: ShareProjectRow[];
  /** 7 列热力图（按本地日对齐；全 0 时整块隐藏） */
  heat: { label: string; tokens: number }[];
}

/** canvas 上的文案（跟随 UI 语言，渲染时由 t 汇出） */
interface ShareTexts {
  periodTitle: string;
  totalCost: string;
  totalTokens: string;
  totalRequests: string;
  topModels: string;
  topProjects: string;
  heatTitle: string;
  footer: string;
}

/** AgentId → 后端 source 字符串（取 sourceMeta 的品牌色/名） */
const AGENT_SOURCE_OF = {
  zai: "zcode",
  codex: "codex",
  claude: "claude",
  cursor: "cursor",
  kimi: "kimi",
} as const;

/**
 * 拉取分享卡片数据：固定近 7 天（含今天）范围，一轮并发取全部来源后
 * 同步聚合。自拉自合（不经 DataCache 30s 轮询），未安装/未登录的
 * Agent 静默跳过，与用量报告页同款容错口径。
 */
async function loadShareData(
  agentVisibility: AgentVisibility,
  pricing: PricingConfig
): Promise<ShareData> {
  const toMs = Date.now();
  const fromMs = startOfLocalDay(toMs) - 6 * DAY_MS;
  const bucket: TrendBucket = "day";

  const safe = async <T,>(task: () => Promise<T>): Promise<T | null> => {
    try {
      return await task();
    } catch {
      // 未安装 / 未登录的 Agent 直接跳过，不阻断卡片生成
      return null;
    }
  };

  const fxRate = await safe(() => getCursorConfig()).then((c) =>
    c && c.usd_cny_rate > 0 ? c.usd_cny_rate : 7.2
  );

  const [zaiStats, codex, claude, kimi, cursor, projects, zaiTrend] =
    await Promise.all([
      agentVisibility.zai
        ? safe(() => fetchStats(fromMs, toMs))
        : Promise.resolve(null),
      agentVisibility.codex
        ? safe(() => fetchCodexUsage(fromMs, toMs, bucket))
        : Promise.resolve(null),
      agentVisibility.claude
        ? safe(() => fetchClaudeUsage(fromMs, toMs, bucket))
        : Promise.resolve(null),
      agentVisibility.kimi
        ? safe(() => fetchKimiUsage(fromMs, toMs, bucket))
        : Promise.resolve(null),
      agentVisibility.cursor
        ? safe(() => fetchCursorUsage(fromMs, toMs))
        : Promise.resolve(null),
      safe(() => getProjects(fromMs, toMs)),
      agentVisibility.zai
        ? safe(() => fetchTrend(fromMs, toMs, bucket))
        : Promise.resolve(null),
    ]);

  // 模型行：逐 Agent 折叠变体后映射（花费按 API 价格表自算，Cursor 用官方标价）
  const agentModelRows = (
    stats: Stats | null,
    agentId: keyof typeof AGENT_SOURCE_OF
  ): ShareModelRow[] => {
    if (!stats) return [];
    const color = sourceMeta(AGENT_SOURCE_OF[agentId]).color;
    return foldModelStatRows(stats.by_model).map((m) => ({
      name: m.model_id,
      color,
      tokens: m.total_tokens,
      costUsd: modelCost(
        m.model_id,
        m.input_tokens,
        m.output_tokens,
        m.cache_read_tokens,
        pricing,
        "usd",
        fxRate
      ),
    }));
  };

  const cursorRows: ShareModelRow[] = cursor
    ? foldCursorModelRows(cursor.by_model).map((m) => ({
        name: m.model,
        color: sourceMeta(AGENT_SOURCE_OF.cursor).color,
        tokens: m.total_tokens,
        costUsd: m.cost_usd,
      }))
    : [];

  const allModelRows = [
    ...agentModelRows(zaiStats, "zai"),
    ...agentModelRows(codex?.stats ?? null, "codex"),
    ...agentModelRows(claude?.stats ?? null, "claude"),
    ...agentModelRows(kimi?.stats ?? null, "kimi"),
    ...cursorRows,
  ];
  const models = [...allModelRows]
    .filter((m) => m.tokens > 0 || m.costUsd > 0)
    .sort((a, b) => b.tokens - a.tokens)
    .slice(0, 5);

  const totalCostUsd = allModelRows.reduce((s, m) => s + m.costUsd, 0);

  // 合计取各来源整体口径（模型行已被 Top5 截断，不能作合计分母）
  let tokens = 0;
  let requests = 0;
  const addOverall = (o: { total_tokens: number; requests: number } | null) => {
    if (!o) return;
    tokens += o.total_tokens;
    requests += o.requests;
  };
  addOverall(zaiStats?.overall ?? null);
  addOverall(codex?.stats.overall ?? null);
  addOverall(claude?.stats.overall ?? null);
  addOverall(kimi?.stats.overall ?? null);
  addOverall(
    cursor?.events
      ? { total_tokens: cursor.events.total_tokens, requests: cursor.events.requests }
      : null
  );

  // 可见 Agent 对应的后端 source 集合（与各 Agent 数据拉取同口径；
  // 不在集合内的来源按不可见处理）
  const visibleSources = new Set<string>(
    (Object.keys(AGENT_SOURCE_OF) as (keyof typeof AGENT_SOURCE_OF)[])
      .filter((id) => agentVisibility[id])
      .map((id) => AGENT_SOURCE_OF[id])
  );

  // 项目 Top5：与卡片其余板块同口径，先按 Agent 可见性重算——不可见来源的
  // token/花费从项目总计中扣除，扣除后无可见用量的项目整个丢弃，再排除未知
  // 项目、按 API 花费降序取前五；全被过滤时项目区块整体隐藏（降级开关）。
  // 避免把用户主动隐藏的 Agent 的项目路径与花费分享出去。
  const projectRows: ShareProjectRow[] = (projects ?? [])
    .map((p) => {
      const visibleAgents = p.by_agent.filter((b) =>
        visibleSources.has(b.source)
      );
      return {
        p,
        tokens: visibleAgents.reduce((s, b) => s + b.tokens, 0),
        costUsd: visibleAgents.reduce((s, b) => s + b.cost_usd, 0),
      };
    })
    .filter(({ p, tokens, costUsd }) => !p.is_unknown && (costUsd > 0 || tokens > 0))
    .sort((a, b) => b.costUsd - a.costUsd)
    .slice(0, 5)
    .map(({ p, tokens, costUsd }) => ({
      name: (p.display_path ?? p.key).split("/").filter(Boolean).pop() ?? p.key,
      tokens,
      costUsd,
    }));

  // 7 列热力图：多来源趋势按 day 标签合并 token（Cursor 官方明细为日粒度，
  // 转成同构 TrendPoint 后直接并入）
  const heatParts: TrendPoint[] = [
    ...(zaiTrend ?? []),
    ...(codex?.trend ?? []),
    ...(claude?.trend ?? []),
    ...(kimi?.trend ?? []),
    ...(cursor
      ? cursor.daily.map((d) => ({
          label: d.date,
          total_tokens: d.total_tokens,
          requests: d.requests,
          cost_cny: d.cost_usd * fxRate,
          cost_usd: d.cost_usd,
        }))
      : []),
  ];
  const heatByLabel = new Map<string, number>();
  for (const p of heatParts) {
    heatByLabel.set(p.label, (heatByLabel.get(p.label) ?? 0) + p.total_tokens);
  }
  const heat: { label: string; tokens: number }[] = [];
  for (let i = 0; i < 7; i++) {
    const label = mdLabel(startOfLocalDay(fromMs) + i * DAY_MS);
    heat.push({ label, tokens: heatByLabel.get(label) ?? 0 });
  }

  return {
    fromMs,
    toMs,
    fxRate,
    totalCostUsd,
    tokens,
    requests,
    models,
    projects: projectRows,
    heat,
  };
}

// ===== Canvas 绘制 =====

function roundRectPath(
  ctx: CanvasRenderingContext2D,
  x: number,
  y: number,
  w: number,
  h: number,
  r: number
) {
  const radius = Math.min(r, w / 2, h / 2);
  ctx.beginPath();
  ctx.moveTo(x + radius, y);
  ctx.lineTo(x + w - radius, y);
  ctx.arcTo(x + w, y, x + w, y + radius, radius);
  ctx.lineTo(x + w, y + h - radius);
  ctx.arcTo(x + w, y + h, x + w - radius, y + h, radius);
  ctx.lineTo(x + radius, y + h);
  ctx.arcTo(x, y + h, x, y + h - radius, radius);
  ctx.lineTo(x, y + radius);
  ctx.arcTo(x, y, x + radius, y, radius);
  ctx.closePath();
}

function setFont(
  ctx: CanvasRenderingContext2D,
  size: number,
  weight = 400
): void {
  ctx.font = `${weight} ${size}px ${FONT_STACK}`;
}

/** 超宽文本截断加省略号 */
function fitText(
  ctx: CanvasRenderingContext2D,
  text: string,
  maxWidth: number
): string {
  if (ctx.measureText(text).width <= maxWidth) return text;
  let out = text;
  while (out.length > 1 && ctx.measureText(out + "…").width > maxWidth) {
    out = out.slice(0, -1);
  }
  return out + "…";
}

function drawSectionTitle(
  ctx: CanvasRenderingContext2D,
  text: string,
  y: number
): void {
  setFont(ctx, 26, 600);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.fillText(text, CARD_PAD, y);
}

function drawBarRow(
  ctx: CanvasRenderingContext2D,
  y: number,
  rowH: number,
  name: string,
  barColor: string,
  dotColor: string,
  share: number,
  valueText: string
): void {
  const nameW = 330;
  const barX = CARD_PAD + nameW;
  const barW = CARD_W - CARD_PAD * 2 - nameW - 220;
  const valueX = CARD_W - CARD_PAD;

  // 名称（圆点 + 文本）
  ctx.fillStyle = dotColor;
  ctx.beginPath();
  ctx.arc(CARD_PAD + 7, y + rowH / 2, 7, 0, Math.PI * 2);
  ctx.fill();
  setFont(ctx, 26, 500);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText(fitText(ctx, name, nameW - 30), CARD_PAD + 26, y + rowH / 2 + 9);

  // 条形轨道 + 占比条
  const barY = y + rowH / 2 - 9;
  roundRectPath(ctx, barX, barY, barW, 18, 9);
  ctx.fillStyle = BAR_TRACK;
  ctx.fill();
  if (share > 0) {
    roundRectPath(
      ctx,
      barX,
      barY,
      Math.max(barW * Math.min(share, 1), 18),
      18,
      9
    );
    const grad = ctx.createLinearGradient(barX, 0, barX + barW, 0);
    grad.addColorStop(0, barColor);
    grad.addColorStop(1, barColor + "aa");
    ctx.fillStyle = grad;
    ctx.fill();
  }

  // 右侧数值
  setFont(ctx, 24, 600);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.textAlign = "right";
  ctx.fillText(valueText, valueX, y + rowH / 2 + 9);
  ctx.textAlign = "left";
}

/**
 * 绘制分享卡片。按内容计算高度并一次性设定画布尺寸（2x 物理像素），
 * 返回卡片逻辑高度。
 */
function drawShareCard(
  canvas: HTMLCanvasElement,
  data: ShareData,
  texts: ShareTexts
): number {
  const ctx = canvas.getContext("2d");
  if (!ctx) return 0;

  const showModels = data.models.length > 0;
  const showProjects = data.projects.length > 0;
  const heatMax = Math.max(...data.heat.map((d) => d.tokens), 1);
  const showHeat = data.heat.some((d) => d.tokens > 0);

  // 先按内容算高度，再一次性设定画布尺寸（重设尺寸会清空画布）
  const HEADER_H = 168;
  const METRICS_H = 188;
  const ROW_H = 62;
  const SECTION_GAP = 44;
  let contentH = CARD_PAD + HEADER_H + METRICS_H;
  if (showModels)
    contentH += SECTION_GAP + 44 + data.models.length * ROW_H + 18;
  if (showProjects)
    contentH += SECTION_GAP + 44 + data.projects.length * ROW_H + 18;
  if (showHeat) contentH += SECTION_GAP + 44 + 148;
  const H = contentH + 96;

  canvas.width = CARD_W * CARD_SCALE;
  canvas.height = Math.round(H * CARD_SCALE);
  ctx.setTransform(CARD_SCALE, 0, 0, CARD_SCALE, 0, 0);

  // 背景：深底渐变 + 顶部天蓝色氛围光（canvas 上模拟深色毛玻璃，不用 backdrop-filter）
  const bg = ctx.createLinearGradient(0, 0, CARD_W, H);
  bg.addColorStop(0, DARK_BG_TOP);
  bg.addColorStop(0.55, DARK_BG_MID);
  bg.addColorStop(1, DARK_BG_BOTTOM);
  ctx.fillStyle = bg;
  ctx.fillRect(0, 0, CARD_W, H);
  const glow = ctx.createRadialGradient(
    CARD_W * 0.18,
    -60,
    40,
    CARD_W * 0.18,
    -60,
    620
  );
  glow.addColorStop(0, "rgba(56, 189, 248, 0.22)");
  glow.addColorStop(1, "rgba(56, 189, 248, 0)");
  ctx.fillStyle = glow;
  ctx.fillRect(0, 0, CARD_W, H * 0.5);

  ctx.textBaseline = "alphabetic";

  // ===== 顶部：品牌 + 周期标题 + 日期范围 =====
  const headY = CARD_PAD;

  // 品牌徽标：渐变圆角方块 + Z 字
  const markSize = 52;
  roundRectPath(ctx, CARD_PAD, headY, markSize, markSize, 14);
  const markGrad = ctx.createLinearGradient(
    CARD_PAD,
    headY,
    CARD_PAD + markSize,
    headY + markSize
  );
  markGrad.addColorStop(0, "#0ea5e9");
  markGrad.addColorStop(1, "#6366f1");
  ctx.fillStyle = markGrad;
  ctx.fill();
  setFont(ctx, 32, 800);
  ctx.fillStyle = "#ffffff";
  ctx.textAlign = "center";
  ctx.fillText("Z", CARD_PAD + markSize / 2, headY + markSize / 2 + 11);
  ctx.textAlign = "left";

  setFont(ctx, 30, 700);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText("ZBar", CARD_PAD + markSize + 18, headY + 35);

  setFont(ctx, 54, 700);
  ctx.fillStyle = TEXT_PRIMARY;
  ctx.fillText(texts.periodTitle, CARD_PAD, headY + 116);

  setFont(ctx, 24, 400);
  ctx.fillStyle = TEXT_SECONDARY;
  ctx.fillText(
    `${mdLabel(data.fromMs).replace("-", ".")} – ${mdLabel(data.toMs).replace("-", ".")}`,
    CARD_PAD,
    headY + 152
  );

  // ===== 主体指标：总花费 / 总 Token / 总请求 =====
  let y = headY + HEADER_H;
  const metricY = y;
  const colW = (CARD_W - CARD_PAD * 2) / 3;

  roundRectPath(
    ctx,
    CARD_PAD,
    metricY,
    CARD_W - CARD_PAD * 2,
    METRICS_H - 30,
    24
  );
  ctx.fillStyle = "rgba(148, 163, 184, 0.08)";
  ctx.fill();

  const metricCell = (
    col: number,
    label: string,
    value: string,
    sub?: string
  ) => {
    const cx = CARD_PAD + col * colW;
    setFont(ctx, 20, 500);
    ctx.fillStyle = TEXT_SECONDARY;
    ctx.fillText(label, cx + 28, metricY + 46);
    setFont(ctx, 52, 700);
    ctx.fillStyle = col === 0 ? ACCENT : TEXT_PRIMARY;
    ctx.fillText(fitText(ctx, value, colW - 56), cx + 28, metricY + 104);
    if (sub) {
      setFont(ctx, 20, 500);
      ctx.fillStyle = TEXT_FAINT;
      ctx.fillText(sub, cx + 28, metricY + 136);
    }
  };
  // 总花费：人民币主展示（按汇率折算，与面板展示习惯一致），美元小字附注
  metricCell(
    0,
    texts.totalCost,
    formatCost(data.totalCostUsd * data.fxRate, "cny"),
    formatCost(data.totalCostUsd, "usd")
  );
  metricCell(1, texts.totalTokens, formatTokens(data.tokens));
  metricCell(2, texts.totalRequests, formatTokens(data.requests));
  y += METRICS_H;

  // ===== 模型 Top5 =====
  if (showModels) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.topModels, y + 30);
    y += 44;
    const maxTokens = Math.max(...data.models.map((m) => m.tokens), 1);
    data.models.forEach((m, i) => {
      drawBarRow(
        ctx,
        y + i * ROW_H,
        ROW_H,
        m.name,
        m.color,
        m.color,
        m.tokens / maxTokens,
        formatTokens(m.tokens)
      );
    });
    y += data.models.length * ROW_H + 18;
  }

  // ===== 项目 Top5 =====
  if (showProjects) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.topProjects, y + 30);
    y += 44;
    const maxCost = Math.max(...data.projects.map((p) => p.costUsd), 0.000001);
    data.projects.forEach((p, i) => {
      drawBarRow(
        ctx,
        y + i * ROW_H,
        ROW_H,
        p.name,
        ACCENT,
        "#818cf8",
        p.costUsd / maxCost,
        formatCost(p.costUsd, "usd")
      );
    });
    y += data.projects.length * ROW_H + 18;
  }

  // ===== 7 天热力图缩影 =====
  if (showHeat) {
    y += SECTION_GAP;
    drawSectionTitle(ctx, texts.heatTitle, y + 30);
    y += 44;
    const gap = 14;
    const cols = data.heat.length;
    const cellW = (CARD_W - CARD_PAD * 2 - gap * (cols - 1)) / cols;
    const cellH = 96;
    data.heat.forEach((d, i) => {
      const cx = CARD_PAD + i * (cellW + gap);
      const intensity = d.tokens / heatMax;
      roundRectPath(ctx, cx, y, cellW, cellH, 12);
      ctx.fillStyle =
        d.tokens > 0
          ? `rgba(56, 189, 248, ${0.18 + intensity * 0.72})`
          : "rgba(148, 163, 184, 0.08)";
      ctx.fill();
      setFont(ctx, 18, 500);
      ctx.fillStyle = TEXT_FAINT;
      ctx.textAlign = "center";
      ctx.fillText(d.label, cx + cellW / 2, y + cellH + 26);
      ctx.textAlign = "left";
    });
    y += cellH + 52;
  }

  // ===== 底部落款 =====
  ctx.strokeStyle = "rgba(148, 163, 184, 0.16)";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(CARD_PAD, H - CARD_PAD + 8);
  ctx.lineTo(CARD_W - CARD_PAD, H - CARD_PAD + 8);
  ctx.stroke();

  setFont(ctx, 22, 500);
  ctx.fillStyle = TEXT_FAINT;
  ctx.textAlign = "right";
  ctx.fillText(texts.footer, CARD_W - CARD_PAD, H - CARD_PAD + 46);
  ctx.textAlign = "left";

  return H;
}

// ===== 弹层组件 =====

interface ShareCardModalProps {
  onClose: () => void;
  pricing: PricingConfig;
  agentVisibility: AgentVisibility;
}

/**
 * 分享卡片弹层：打开即生成近 7 天卡片（实时 canvas 预览），
 * 「保存图片」经系统对话框写盘（用户取消返回 null，静默处理）。
 */
export function ShareCardModal({
  onClose,
  pricing,
  agentVisibility,
}: ShareCardModalProps) {
  const { t } = useI18n();
  const [data, setData] = useState<ShareData | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);
  const [saving, setSaving] = useState(false);
  const [status, setStatus] = useState<string | null>(null);
  const [nonce, setNonce] = useState(0);
  const canvasRef = useRef<HTMLCanvasElement | null>(null);

  const texts = useMemo<ShareTexts>(
    () => ({
      periodTitle: t("share.periodTitle"),
      totalCost: t("share.totalCost"),
      totalTokens: t("share.totalTokens"),
      totalRequests: t("share.totalRequests"),
      topModels: t("share.topModels"),
      topProjects: t("share.topProjects"),
      heatTitle: t("share.heatTitle"),
      footer: t("share.footer"),
    }),
    [t]
  );

  useEffect(() => {
    let alive = true;
    setLoading(true);
    setError(null);
    loadShareData(agentVisibility, pricing)
      .then((d) => {
        if (alive) setData(d);
      })
      .catch((e) => {
        if (alive) setError(String(e));
      })
      .finally(() => {
        if (alive) setLoading(false);
      });
    return () => {
      alive = false;
    };
  }, [agentVisibility, pricing, nonce]);

  // 数据/文案就绪后重绘（canvas 挂载于非 loading / 非 error 分支）
  useEffect(() => {
    if (!loading && !error && data && canvasRef.current) {
      drawShareCard(canvasRef.current, data, texts);
    }
  }, [data, texts, loading, error]);

  const handleSave = () => {
    const canvas = canvasRef.current;
    if (!canvas || saving) return;
    setSaving(true);
    setStatus(null);
    canvas.toBlob(async (blob) => {
      if (!blob) {
        setSaving(false);
        return;
      }
      try {
        const buf = await blob.arrayBuffer();
        const bytes = Array.from(new Uint8Array(buf));
        const name = `zbar-share-${ymdLabel(Date.now())}.png`;
        const path = await saveShareImage(bytes, name);
        // 用户取消（返回 null）静默处理；成功提示保存路径
        if (path) setStatus(t("share.saved", { path }));
      } catch (e) {
        setStatus(t("share.saveFail", { msg: String(e) }));
      } finally {
        setSaving(false);
      }
    }, "image/png");
  };

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-black/40">
      <div className="bg-elevated border border-slate-900/10 rounded-2xl shadow-xl p-3 w-[min(560px,94vw)] max-h-[92vh] flex flex-col">
        <div className="flex items-center justify-between mb-2 shrink-0">
          <span className="text-[12px] font-semibold text-slate-900">
            {t("share.title")}
          </span>
          <button
            onClick={onClose}
            className="toolbar-btn"
            title={t("share.close")}
          >
            ×
          </button>
        </div>
        <div className="flex-1 min-h-0 overflow-y-auto rounded-xl bg-slate-950/85 p-2 overscroll-contain">
          {loading ? (
            <div className="py-16 text-center text-xs text-slate-400">
              {t("share.loading")}
            </div>
          ) : error ? (
            <div className="py-16 text-center text-xs text-rose-300 break-all px-4">
              {t("share.loadFail", { msg: error })}
            </div>
          ) : (
            <canvas
              ref={canvasRef}
              className="w-full h-auto rounded-lg"
              aria-label={t("share.title")}
            />
          )}
        </div>
        <div className="flex items-center gap-1.5 mt-2 shrink-0">
          <span className="flex-1 min-w-0 text-[9px] text-slate-600 truncate">
            {status}
          </span>
          <BtnSecondary
            onClick={() => {
              setStatus(null);
              setNonce((n) => n + 1);
            }}
            disabled={loading}
            className="text-[10px]!"
          >
            {t("share.regenerate")}
          </BtnSecondary>
          <BtnPrimary
            onClick={handleSave}
            disabled={loading || saving || !!error || !data}
            className="text-[10px]!"
          >
            {saving ? t("share.saving") : t("share.save")}
          </BtnPrimary>
        </div>
      </div>
    </div>
  );
}

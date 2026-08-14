import { useCallback, useEffect, useState } from "react";
import type {
  CostResult,
  Currency,
  DeviceInfo,
  PricingConfig,
  QuotaSnapshot,
  RemoteSnapshot,
  RemoteUsage,
  Stats,
  SyncConfig,
} from "./types";
import {
  computeCost,
  fetchStats,
  getQuotaHistory,
  getSyncConfig,
  listRemoteDevices,
  remoteSnapshots,
  remoteUsage,
  saveReport,
} from "./api";
import { formatCost, formatTokens } from "./format";
import { writeText } from "@tauri-apps/plugin-clipboard-manager";
import {
  computeRemoteCost,
  mergeCost,
  mergeStats,
  remoteToStats,
} from "./merge";

interface Props {
  onBack: () => void;
  pricing: PricingConfig;
}

type ReportKind = "daily" | "weekly";

/** 本地日期 YYYY-MM-DD（避免 UTC 偏移） */
function localDateStr(ms: number): string {
  const d = new Date(ms);
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

export function ReportPanel({ onBack, pricing }: Props) {
  const [kind, setKind] = useState<ReportKind>("daily");
  const [stats, setStats] = useState<Stats | null>(null);
  const [cost, setCost] = useState<CostResult | null>(null);
  const [snaps, setSnaps] = useState<QuotaSnapshot[]>([]);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [doneFlash, setDoneFlash] = useState<string | null>(null);

  // 多设备同步相关
  const [syncConfig, setSyncConfig] = useState<SyncConfig | null>(null);
  const [remoteDevices, setRemoteDevices] = useState<DeviceInfo[]>([]);
  const [deviceFilter, setDeviceFilter] = useState<string>("all");
  const syncEnabled = !!syncConfig?.enabled && !!syncConfig.device_token;

  // 货币：报告里同时展示 CNY + USD，这里取 pricing 默认（无需 UI 切换）
  const currency: Currency = "cny";

  // 初次读取同步配置 + 设备列表
  useEffect(() => {
    getSyncConfig()
      .then((cfg) => {
        setSyncConfig(cfg);
        if (cfg.enabled && cfg.device_token) {
          listRemoteDevices()
            .then(setRemoteDevices)
            .catch(() => {});
        }
      })
      .catch(() => {});
  }, []);

  const load = useCallback(async () => {
    setLoading(true);
    setError(null);
    const now = Date.now();
    const from =
      kind === "daily"
        ? new Date().setHours(0, 0, 0, 0)
        : now - 7 * 86400000;

    // 数据来源：all=本地+远端(排除本机)；local=仅本地；具体id=仅远端该设备
    const wantLocal = deviceFilter === "all" || deviceFilter === "local";
    const wantRemote =
      syncEnabled &&
      (deviceFilter === "all" ||
        (deviceFilter !== "local" && deviceFilter !== "all"));

    const opts =
      syncConfig &&
      (deviceFilter === "all"
        ? { excludeDevice: syncConfig.device_id }
        : { devices: deviceFilter });

    const tasks: Promise<unknown>[] = [];

    // 本地 stats + cost
    let localStats: Stats | null = null;
    let localCost: CostResult | null = null;
    if (wantLocal) {
      tasks.push(
        fetchStats(from, now).then((s) => (localStats = s)),
        computeCost(from, now).then((c) => (localCost = c))
      );
    }

    // 远端 stats（remote_usage）—— 远端无 cost，前端用 pricing 自算
    let remote: RemoteUsage | null = null;
    if (wantRemote && syncConfig && opts) {
      tasks.push(
        remoteUsage(from, now, "day", opts)
          .then((r) => (remote = r))
          .catch((e) => {
            // 具体远端设备失败透出；全部模式静默降级
            if (deviceFilter !== "all") throw e;
          })
      );
    }

    // 本地快照 + 远端快照
    let localSnaps: QuotaSnapshot[] = [];
    let remoteSnaps: RemoteSnapshot[] = [];
    if (wantLocal) {
      tasks.push(
        getQuotaHistory().then((h) => (localSnaps = h.filter((x) => x.ts >= from)))
      );
    }
    if (wantRemote && syncConfig && opts) {
      tasks.push(
        remoteSnapshots(from, now, opts)
          .then((s) => (remoteSnaps = s))
          .catch(() => {
            if (deviceFilter !== "all") throw new Error("远端快照获取失败");
          })
      );
    }

    try {
      await Promise.all(tasks);

      // 合并 stats + cost
      let mergedStats: Stats | null = null;
      let mergedCost: CostResult | null = null;
      if (wantLocal && localStats && remote) {
        mergedStats = mergeStats(localStats, remote);
        mergedCost = mergeCost(localCost, remote, pricing);
      } else if (wantLocal && localStats) {
        mergedStats = localStats;
        mergedCost = localCost;
      } else if (remote) {
        mergedStats = remoteToStats(remote);
        mergedCost = computeRemoteCost(remote, pricing);
      }

      setStats(mergedStats);
      setCost(mergedCost);

      // 合并快照：本地 + 远端（额度是账户级，多设备采样可互补补全）
      // 远端快照去掉 device_id 字段以匹配 QuotaSnapshot 结构
      const mergedSnaps: QuotaSnapshot[] = [
        ...(wantLocal ? localSnaps : []),
        ...remoteSnaps.map((s) => ({
          ts: s.ts,
          level: s.level,
          weekly_pct: s.weekly_pct,
          weekly_reset: s.weekly_reset,
          hour5_pct: s.hour5_pct,
          mcp_pct: s.mcp_pct,
          mcp_used: s.mcp_used,
          mcp_total: s.mcp_total,
        })),
      ].sort((a, b) => a.ts - b.ts);
      setSnaps(mergedSnaps);
    } catch (e) {
      setError(String(e));
    } finally {
      // 关键：无论成功/失败都要结束 loading，否则页面永远停在"生成中…"
      setLoading(false);
    }
  }, [kind, deviceFilter, syncConfig, syncEnabled, pricing, currency]);

  useEffect(() => {
    load();
  }, [load]);

  // 组装 Markdown 文本
  const markdown = buildMarkdown(kind, stats, cost, snaps, currency);
  const filename = `${kind === "daily" ? "日报" : "周报"}-${localDateStr(
    Date.now()
  )}.md`;

  const handleCopy = async () => {
    try {
      await writeText(markdown);
      setDoneFlash("已复制到剪贴板 ✓");
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSave = async () => {
    setError(null);
    try {
      await saveReport(markdown, filename);
      setDoneFlash("已保存并在文件夹打开 ✓");
      setTimeout(() => setDoneFlash(null), 1800);
    } catch (e) {
      setError(String(e));
    }
  };

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
          <h1 className="text-[13px] font-semibold text-slate-900/90">
            用量报告
          </h1>
          <span className="w-10" />
        </div>
        {/* 报告类型切换 */}
        <div className="flex gap-1 mb-2">
          {(["daily", "weekly"] as ReportKind[]).map((k) => (
            <button
              key={k}
              onClick={() => setKind(k)}
              className={`px-2 py-0.5 rounded-md text-[11px] transition-colors ${
                kind === k
                  ? "bg-sky-500 text-white"
                  : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10 hover:text-slate-900/80"
              }`}
            >
              {k === "daily" ? "📅 日报（今日）" : "📅 周报（近 7 天）"}
            </button>
          ))}
        </div>
        {/* 设备筛选器：仅在同步启用时显示 */}
        {syncEnabled && (
          <div className="flex items-center gap-1.5">
            <span className="text-[10px] text-slate-700/45 shrink-0">设备</span>
            <select
              value={deviceFilter}
              onChange={(e) => setDeviceFilter(e.target.value)}
              className="num flex-1 px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-[10px] text-slate-900/80 focus:outline-none focus:border-sky-400/60"
            >
              <option value="all">全部（汇总）</option>
              <option value="local">
                本机{syncConfig?.device_name ? `（${syncConfig.device_name}）` : ""}
              </option>
              {remoteDevices
                .filter((d) => d.device_id !== syncConfig?.device_id)
                .map((d) => (
                  <option key={d.device_id} value={d.device_id}>
                    {d.device_name}（{d.device_id.slice(0, 6)}）
                  </option>
                ))}
            </select>
          </div>
        )}
      </div>

      {error && (
        <div className="mx-3 mt-2 px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
          {error}
        </div>
      )}

      {/* Markdown 预览 */}
      <div className="flex-1 overflow-auto px-3.5 py-2.5">
        {loading ? (
          <div className="text-xs text-slate-700/55 text-center py-8">
            生成中…
          </div>
        ) : (
          <pre className="num text-[11px] leading-relaxed text-slate-900/85 whitespace-pre-wrap break-words">
            {markdown}
          </pre>
        )}
      </div>

      {/* 底部操作 */}
      <div className="px-3.5 py-2 border-t border-slate-900/10 flex items-center justify-between">
        <span className="text-[10px] text-slate-700/45 truncate">
          {doneFlash || filename}
        </span>
        <div className="flex gap-1.5">
          <button
            onClick={handleCopy}
            disabled={loading}
            className="text-xs px-2.5 py-0.5 rounded-md bg-slate-900/5 text-slate-700/75 hover:bg-slate-900/10 disabled:opacity-40 transition-colors"
          >
            复制
          </button>
          <button
            onClick={handleSave}
            disabled={loading}
            className="text-xs px-2.5 py-0.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
          >
            保存为文件
          </button>
        </div>
      </div>
    </div>
  );
}

/** 组装 Markdown 报告文本 */
function buildMarkdown(
  kind: ReportKind,
  stats: Stats | null,
  cost: CostResult | null,
  snaps: QuotaSnapshot[],
  currency: Currency
): string {
  const now = Date.now();
  const title = kind === "daily" ? "日报" : "周报";
  const dateRange =
    kind === "daily"
      ? localDateStr(now)
      : `${localDateStr(now - 7 * 86400000)} ~ ${localDateStr(now)}`;

  if (!stats || !cost) {
    return `📊 ZBar 用量${title} · ${dateRange}\n\n（暂无数据）`;
  }

  const lines: string[] = [];
  lines.push(`📊 ZBar 用量${title} · ${dateRange}`);
  lines.push("");

  // 概览
  const cny = cost.total_cny;
  const usd = cost.total_usd;
  lines.push(
    `💰 总花费 ${formatCost(cny, "cny")}（${formatCost(
      usd,
      "usd"
    )}）｜ Token ${formatTokens(stats.overall.total_tokens)}｜请求 ${stats.overall.requests.toLocaleString()} 次`
  );
  lines.push("");

  // 模型 TOP（按当前货币花费降序）
  const perModel = currency === "cny" ? cost.per_model_cny : cost.per_model_usd;
  // 同一 model_id 多设备合并后可能有多条，先按 model_id 聚合
  const modelAgg = new Map<string, number>();
  for (const m of perModel) {
    modelAgg.set(m.model_id, (modelAgg.get(m.model_id) ?? 0) + m.cost);
  }
  const rows = stats.by_model
    .map((m) => ({ m, c: modelAgg.get(m.model_id) ?? 0 }))
    .sort((a, b) => b.c - a.c);
  const totalCost = rows.reduce((s, r) => s + r.c, 0);
  const top = rows.slice(0, 3);
  if (top.length > 0) {
    lines.push("🏆 模型消耗 TOP3");
    top.forEach((r, i) => {
      const pct = totalCost > 0 ? Math.round((r.c / totalCost) * 100) : 0;
      const tok = r.m.total_tokens;
      lines.push(
        `${i + 1}. ${r.m.model_id}  ${formatCost(r.c, currency)} (${pct}%)  ${formatTokens(
          tok
        )} tok`
      );
    });
    lines.push("");
  }

  // 额度（仅当有快照）
  if (snaps.length > 0) {
    const weeklyPeak = Math.max(...snaps.map((s) => s.weekly_pct));
    const hour5Peak = Math.max(...snaps.map((s) => s.hour5_pct));
    const last = snaps[snaps.length - 1];
    const reset = last?.weekly_reset;
    const resetStr = reset
      ? `距重置 ${Math.max(
          0,
          Math.ceil((reset - now) / 86400000)
        )} 天`
      : "—";
    lines.push("📈 周额度");
    lines.push(`- 峰值 ${weeklyPeak}%｜${resetStr}`);
    lines.push(`- 5h 窗口峰值 ${hour5Peak}%`);
    if (last?.mcp_total) {
      lines.push(`- MCP 月度 ${last.mcp_used ?? 0}/${last.mcp_total}`);
    }
    lines.push("");
  }

  // 提示
  if (top.length > 0) {
    lines.push("⚠ 提示");
    const first = top[0];
    const firstPct =
      totalCost > 0 ? Math.round((first.c / totalCost) * 100) : 0;
    if (firstPct >= 60) {
      lines.push(
        `- ${first.m.model_id} 占 ${firstPct}%，简单任务可考虑更便宜的模型`
      );
    } else {
      lines.push("- 用量分布均衡，无明显偏科");
    }
  }

  lines.push("");
  lines.push(`> 由 ZBar 自动生成 · ${new Date().toLocaleString("zh-CN")}`);
  return lines.join("\n");
}

import { useEffect, useState } from "react";
import { useDataCache } from "./DataCache";
import { formatCountdownCore, levelLabel } from "./format";
import { remainingGradient, remainingTextColor } from "./widgets";
import { useI18n } from "./i18n";
import {
  SwitchAccountButton,
  SwitchConfirmOverlay,
  useAccountSwitch,
} from "./accountSwitch";
import type { AccountQuotaEntry } from "./types";

export function QuotaPanel() {
  // 额度数据由全局 DataProvider 统一预加载 + 30s 定时刷新，
  // 此组件仅负责展示（纯展示层），不再自己请求。
  const { quota, quotaError, todayDelta, refreshQuota, accountQuotas } =
    useDataCache();
  // 卡片内嵌账号切换（全部账号区各非当前账号行）
  const sw = useAccountSwitch();
  const { t } = useI18n();
  const [now, setNow] = useState(Date.now());

  // 每秒更新倒计时显示
  useEffect(() => {
    const tickTimer = setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(tickTimer);
  }, []);

  // 本地凭证缺失（未在 ZCode 客户端登录 Coding Plan）→ 显示登录引导
  // 注意：正则匹配 Rust 后端返回的中文错误串（quota.rs 的固定前缀文案），仅做布尔分支，不能翻译
  if (quotaError && quotaError.includes("未找到 ZCode Coding Plan 凭证")) {
    return (
      <div className="mb-1 rounded-lg bg-sky-500/10 border border-sky-500/20 px-2.5 py-1.5">
        <span className="text-[11px] text-sky-700/80">{t("quota.title")}</span>
        <p className="text-[10px] text-slate-700/50 mt-0.5">
          {t("quota.configHint")}
        </p>
      </div>
    );
  }

  // 其他错误：仍有多账号数据时降级渲染整卡（当前账号行报错、其余账号照常展示
  // ——当前凭证失效时恰恰是用户最需要看其他账号、考虑切换的时刻）
  const quotaFailed = quotaError != null || !quota;
  if (quotaFailed && accountQuotas.length < 2) {
    return (
      <div className="mb-1 rounded-lg bg-amber-500/10 border border-amber-500/20 px-2.5 py-1.5">
        <span className="text-[10px] text-amber-700/80">
          {quotaError ? t("quota.failed", { msg: quotaError }) : t("common.loading")}
        </span>
      </div>
    );
  }

  const planBadge = quota?.level ? levelLabel(quota.level) : "—";

  return (
    <div className="relative mb-1 card-base rounded-2xl px-3 py-2.5">
      {/* 标题行 */}
      <div className="flex items-center justify-between mb-1.5">
        <div className="flex items-center gap-1.5">
          <span className="text-[10px] uppercase tracking-wide text-slate-700/55">
            Coding Plan
          </span>
          <span className="px-1.5 py-0 rounded text-[9px] font-semibold bg-violet-500/15 text-violet-700">
            {planBadge}
          </span>
        </div>
        <button
          onClick={refreshQuota}
          className="text-slate-700/40 hover:text-slate-900/70 text-[11px] transition-colors"
          title={t("quota.refresh")}
        >
          ↻
        </button>
      </div>

      {/* 当前账号额度（失败时此处留空，下方"全部账号"区当前账号行显示错误） */}
      {quotaFailed && (
        <p className="text-[10px] text-amber-700/80">
          {quotaError ? t("quota.failed", { msg: quotaError }) : t("common.loading")}
        </p>
      )}

      {/* 5小时窗口 */}
      {quota?.hour5 && (
        <QuotaBar
          label={t("common.hour5")}
          usedPct={quota.hour5.percentage}
          resetAt={quota.hour5.nextResetTime}
          now={now}
        />
      )}

      {/* 每周 */}
      {quota?.weekly && (
        <QuotaBar
          label={t("common.weekly")}
          usedPct={quota.weekly.percentage}
          resetAt={quota.weekly.nextResetTime}
          now={now}
          className="mt-1.5"
          delta={todayDelta}
        />
      )}

      {/* MCP 月度额度 */}
      {quota?.mcp && (
        <McpBar
          usedPct={quota.mcp.percentage}
          resetAt={quota.mcp.nextResetTime}
          now={now}
          currentValue={quota.mcp.currentValue}
          total={quota.mcp.usage}
          className="mt-1.5"
        />
      )}

      {/* 全部账号：≥2 个快照时逐账号展示（仅 1 个时与上方重复，不显示）。
          当前账号条目已由 DataCache 用 30s live quota 覆盖，其余为 5 分钟一轮。
          列表限高内滚：账号多时不能把 zai 标签的统计主内容挤出可视区。
          非当前账号行带「切换」按钮（确认后退出并重启 ZCode）。 */}
      {accountQuotas.length >= 2 && (
        <div className="mt-2 pt-1.5 border-t border-slate-900/6">
          <div className="text-[9px] uppercase tracking-wide text-slate-700/45 mb-1">
            {t("quota.allAccounts")}
          </div>
          <div className="max-h-36 overflow-y-auto overscroll-contain space-y-1.5">
            {accountQuotas.map((e) => (
              <AccountQuotaRow
                key={e.id}
                entry={e}
                onSwitch={e.is_current ? undefined : () => sw.request(e)}
                switchDisabled={sw.switching}
              />
            ))}
          </div>
        </div>
      )}

      {/* 切换结果反馈：成功绿色短暂展示，失败红字保留 */}
      {sw.notice && (
        <p
          className={`text-[9px] mt-1.5 leading-relaxed break-all ${
            sw.notice.kind === "ok" ? "text-emerald-600" : "text-rose-600"
          }`}
        >
          {sw.notice.text}
        </p>
      )}

      {/* 切换确认浮层（覆盖整卡） */}
      {sw.confirming && (
        <SwitchConfirmOverlay
          account={sw.confirming}
          switching={sw.switching}
          onConfirm={sw.confirm}
          onCancel={sw.cancel}
        />
      )}
    </div>
  );
}

/** 多账号列表单行：名称 + 当前 pill + 等级徽标（sky，与汇总页 ZCode 徽标同色；
 *  violet 只留给"当前"一个语义）+ 每周剩余（条）+ 5小时剩余小字 + 切换按钮 */
function AccountQuotaRow({
  entry,
  onSwitch,
  switchDisabled,
}: {
  entry: AccountQuotaEntry;
  /** 非当前账号时传入（点击弹切换确认）；当前账号为 undefined */
  onSwitch?: () => void;
  switchDisabled?: boolean;
}) {
  const { t } = useI18n();
  const { display_name, is_current, quota, error } = entry;

  if (!quota) {
    return (
      <div className="flex items-center justify-between gap-2" title={error ?? undefined}>
        <span className="flex items-center gap-1.5 min-w-0">
          <span className="text-[10px] text-slate-900/80 truncate">{display_name}</span>
          {is_current && (
            <span className="shrink-0 text-[8px] px-1 py-px rounded-full bg-violet-500/10 text-violet-600">
              {t("settings.accountsCurrent")}
            </span>
          )}
        </span>
        <span className="text-[9px] text-rose-500/90 shrink-0">
          ⚠ {t("quota.quotaFail")}
        </span>
      </div>
    );
  }

  // 每周数据缺失时不得当作"剩余 100%"渲染（接口异常/未开通），整行数值与条都不出
  const weeklyRemain = quota.weekly
    ? Math.max(0, 100 - quota.weekly.percentage)
    : null;
  const hour5RemainPct = quota.hour5
    ? Math.max(0, 100 - quota.hour5.percentage)
    : null;

  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="flex items-center gap-1.5 min-w-0">
          <span className="text-[10px] text-slate-900/80 truncate">{display_name}</span>
          {is_current && (
            <span className="shrink-0 text-[8px] px-1 py-px rounded-full bg-violet-500/10 text-violet-600">
              {t("settings.accountsCurrent")}
            </span>
          )}
          {quota.level && (
            <span className="shrink-0 px-1 py-px rounded text-[8px] font-semibold bg-sky-500/12 text-sky-700">
              {levelLabel(quota.level)}
            </span>
          )}
        </span>
        {onSwitch && (
          <SwitchAccountButton onClick={onSwitch} disabled={switchDisabled} />
        )}
        {weeklyRemain != null && (
          <span
            className="num text-[10px] font-semibold shrink-0 whitespace-nowrap"
            style={{ color: remainingTextColor(weeklyRemain) }}
          >
            {t("quota.weekShort")} {Math.round(weeklyRemain)}%
          </span>
        )}
      </div>
      {weeklyRemain != null && (
        <div className="h-1 rounded-full bg-slate-900/8 overflow-hidden">
          <div
            className="h-full rounded-full transition-all duration-500"
            style={{
              width: `${weeklyRemain}%`,
              background: remainingGradient(weeklyRemain),
            }}
          />
        </div>
      )}
      {hour5RemainPct != null && (
        <div className="num text-[9px] text-slate-700/45 mt-0.5">
          {t("quota.hour5Short")} {Math.round(hour5RemainPct)}%
        </div>
      )}
    </div>
  );
}

/** 单条额度进度条（剩余版：填充剩余量，颜色随剩余渐变） */
function QuotaBar({
  label,
  usedPct,
  resetAt,
  now,
  className = "",
  delta,
}: {
  label: string;
  usedPct: number;
  resetAt: number | null;
  now: number;
  className?: string;
  /** 今日增量：[增量百分比, 采样数]。仅 weekly 传入 */
  delta?: [number, number] | null;
}) {
  const { t } = useI18n();
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 今日增量徽标：采样不足(<2)不显示数字
  const deltaPct = delta ? delta[0] : 0;
  const hasDelta = delta != null && delta[1] >= 2 && deltaPct > 0;

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_3.25rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">{label}</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {t("common.refreshIn", { time: formatCountdownCore(resetAt - now) })}
          </span>
        ) : (
          <span />
        )}
        <span
          className="num font-semibold text-right"
          style={{ color: remainingTextColor(remaining) }}
        >
          {t("common.remaining", { pct: Math.round(remaining) })}
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {/* 今日增量 */}
      {hasDelta && (
        <div className="text-[9px] mt-0.5">
          <span className="num text-slate-700/50">
            {t("quota.todayDelta", { pct: deltaPct })}
          </span>
        </div>
      )}
    </div>
  );
}

/** MCP 月度额度条：与 QuotaBar 同款（剩余版）。
 *  - 中间列显示刷新倒计时（若接口无 nextResetTime 则留空）
 *  - 进度条下方副信息行：绝对值「已用 / 总量」 */
function McpBar({
  usedPct,
  resetAt,
  now,
  currentValue,
  total,
  className = "",
}: {
  usedPct: number;
  resetAt?: number | null;
  now: number;
  currentValue?: number;
  total?: number;
  className?: string;
}) {
  const { t } = useI18n();
  const used = Math.min(Math.max(usedPct, 0), 100);
  const remaining = 100 - used;

  // 有绝对值时显示 "已用 / 总量"
  const hasAbs =
    typeof currentValue === "number" && typeof total === "number";

  return (
    <div className={className}>
      <div className="grid grid-cols-[3.25rem_minmax(0,1fr)_3.25rem] items-center gap-2 text-[10px] mb-0.5">
        <span className="text-slate-700/60">MCP</span>
        {resetAt && resetAt > now ? (
          <span className="num text-slate-700/40 text-right pr-1 whitespace-nowrap">
            ↻ {t("common.refreshIn", { time: formatCountdownCore(resetAt - now) })}
          </span>
        ) : (
          <span />
        )}
        <span
          className="num font-semibold text-right"
          style={{ color: remainingTextColor(remaining) }}
        >
          {t("common.remaining", { pct: Math.round(remaining) })}
        </span>
      </div>
      <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
        <div
          className="h-full rounded-full transition-all duration-500"
          style={{
            width: `${remaining}%`,
            background: remainingGradient(remaining),
          }}
        />
      </div>
      {hasAbs && (
        <div className="text-[9px] text-slate-700/50 mt-0.5 num">
          {currentValue} / {total}
        </div>
      )}
    </div>
  );
}

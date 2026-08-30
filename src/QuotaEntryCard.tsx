/**
 * 单条凭证的配额卡（共享组件）：从 GenericQuotaPanel 提取，供通用额度
 * 面板与 Claude 面板的「其他账号」区两处复用，避免组件复制。
 * 渲染结构：备注名 + 套餐/状态徽章 + 窗口进度条 + 按量余额 + 错误信息。
 * 提取时保持原实现零变化（样式、文案键、渲染条件与原文件一致）。
 */
import type { MessageKey } from "./i18n";
import { useI18n } from "./i18n";
import type { ProviderQuotaEntry } from "./types";
import { ProgressBar, remainingGradient, remainingTextColor } from "./widgets";
import { formatCountdownCore, formatResetStamp } from "./format";

/** 凭证状态 → 徽章文案与配色（ok 为正常态，不渲染徽章） */
function statusBadge(
  status: ProviderQuotaEntry["status"]
): { cls: string; key: MessageKey } | null {
  switch (status) {
    case "expired":
      return { cls: "bg-amber-500/12 text-amber-700", key: "credentials.entryExpired" };
    case "error":
      return { cls: "bg-rose-500/12 text-rose-600", key: "credentials.entryError" };
    case "pending":
      return { cls: "bg-slate-900/8 text-slate-600", key: "credentials.entryPending" };
    default:
      return null;
  }
}

/**
 * 额度窗口 key → i18n 标题键映射：Rust 各 provider 模块下发的 window.title
 * 为硬编码中文，英文界面会混入中文，渲染时按 key 优先取 i18n 文案。
 * 用 Record 收窄动态键类型（MessageKey 为静态联合类型，不能模板字符串直传
 * t()）；未收录的 key（新增 provider / 新窗口形态）回落 Rust 下发的 title，
 * 保证不渲染空标题。key 清单 = 全仓 Rust 侧 ProviderQuotaWindow 的 key 参数。
 */
const WINDOW_TITLE_KEYS: Record<string, MessageKey> = {
  hour5: "credentials.windowTitle.hour5",
  weekly: "credentials.windowTitle.weekly",
  monthly: "credentials.windowTitle.monthly",
  interval: "credentials.windowTitle.interval",
  credits: "credentials.windowTitle.credits",
  quota: "credentials.windowTitle.quota",
  fuel: "credentials.windowTitle.fuel",
  sub_credits: "credentials.windowTitle.sub_credits",
  topup_credits: "credentials.windowTitle.topup_credits",
  opus_weekly: "credentials.windowTitle.opus_weekly",
  sonnet_weekly: "credentials.windowTitle.sonnet_weekly",
  extra_usage: "credentials.windowTitle.extra_usage",
  pro: "credentials.windowTitle.pro",
  flash: "credentials.windowTitle.flash",
  auto: "credentials.windowTitle.auto",
  api: "credentials.windowTitle.api",
};

/** 单条凭证的配额卡：备注名 + 套餐/状态徽章 + 窗口进度条 + 余额 + 错误信息 */
export function QuotaEntryCard({
  entry,
  accent,
  resetDisplay,
  now,
}: {
  entry: ProviderQuotaEntry;
  accent: string;
  resetDisplay: { countdown: boolean; datetime: boolean };
  now: number;
}) {
  const { t } = useI18n();
  const badge = statusBadge(entry.status);
  const isError = entry.status === "error" || entry.status === "expired";
  // 状态点语义与徽章一致：error 红 / expired amber，正常态保持品牌色
  const dotColor =
    entry.status === "error"
      ? "#f43f5e"
      : entry.status === "expired"
        ? "#f59e0b"
        : accent;
  // error/expired 时错误行已是主要信息，更新时间降级为 title 属性
  const updatedAtTitle = t("credentials.updatedAt", {
    time: formatResetStamp(entry.updatedAt),
  });

  return (
    <div className="card-base rounded-2xl px-3 py-2.5">
      {/* 标题行：备注名 + 套餐徽章 + 状态徽章 + 更新时间 */}
      <div
        className="flex items-center justify-between gap-2 mb-1.5"
        title={isError ? updatedAtTitle : undefined}
      >
        <div className="flex items-center gap-1.5 min-w-0">
          <span
            className="w-1.5 h-1.5 rounded-full shrink-0"
            style={{ background: dotColor }}
          />
          <span className="text-[10px] font-medium text-slate-900/85 truncate">
            {entry.label}
          </span>
          {entry.planName && (
            <span
              className="shrink-0 px-1 py-px rounded text-[8px] font-semibold"
              style={{ background: `${accent}1f`, color: accent }}
            >
              {entry.planName}
            </span>
          )}
          {badge && (
            <span className={`shrink-0 px-1.5 py-px rounded-full text-[8px] font-medium ${badge.cls}`}>
              {t(badge.key)}
            </span>
          )}
        </div>
        {!isError && (
          <span className="num text-[9px] text-slate-500/70 shrink-0">
            {formatResetStamp(entry.updatedAt)}
          </span>
        )}
      </div>

      {/* 错误 / 过期：展示原因行 */}
      {isError && entry.message && (
        <p
          className="text-[10px] text-rose-600/90 leading-relaxed mb-1.5 break-all"
          title={entry.message}
        >
          ⚠ {entry.message}
        </p>
      )}

      {/* 用量窗口（标题优先取 key 映射的 i18n 文案，未收录回落 Rust title） */}
      {entry.windows.length > 0 && (
        <div className="space-y-1.5">
          {entry.windows.map((w) => {
            const titleKey = WINDOW_TITLE_KEYS[w.key];
            return (
              <QuotaWindowBar
                key={w.key}
                title={titleKey ? t(titleKey) : w.title}
                usedPercent={w.usedPercent}
                used={w.used}
                total={w.total}
                unit={w.unit}
                resetsAt={w.resetsAt}
                resetDisplay={resetDisplay}
                now={now}
              />
            );
          })}
        </div>
      )}

      {/* 按量余额（充值型 provider） */}
      {entry.balance && (
        <div className="mt-1.5 pt-1.5 border-t border-slate-900/6 flex items-baseline gap-1.5 flex-wrap">
          <span className="text-[9px] text-slate-500">
            {t("credentials.balance")}
          </span>
          <span className="num text-[13px] font-semibold text-slate-900/85">
            {entry.balance.currency}
            {entry.balance.amount.toFixed(2)}
          </span>
          {(entry.balance.granted != null || entry.balance.toppedUp != null) && (
            <span className="num text-[9px] text-slate-500/80">
              {entry.balance.granted != null &&
                t("credentials.balanceGranted", {
                  amount: `${entry.balance.currency}${entry.balance.granted.toFixed(2)}`,
                })}
              {entry.balance.granted != null && entry.balance.toppedUp != null && " · "}
              {entry.balance.toppedUp != null &&
                t("credentials.balanceToppedUp", {
                  amount: `${entry.balance.currency}${entry.balance.toppedUp.toFixed(2)}`,
                })}
            </span>
          )}
        </div>
      )}
    </div>
  );
}

/** 单个用量窗口：标题 + 进度条 + 数值 + 重置倒计时（展示偏好跟随设置页） */
function QuotaWindowBar({
  title,
  usedPercent,
  used,
  total,
  unit,
  resetsAt,
  resetDisplay,
  now,
}: {
  title: string;
  usedPercent?: number;
  used?: number;
  total?: number;
  unit?: string;
  resetsAt?: number;
  resetDisplay: { countdown: boolean; datetime: boolean };
  now: number;
}) {
  const { t } = useI18n();
  const pct = usedPercent ?? (total ? (used ?? 0) / total * 100 : undefined);
  const hasBar = pct != null && isFinite(pct);
  const remain = hasBar ? Math.max(0, 100 - Math.min(Math.max(pct, 0), 100)) : 100;

  // 数值文本：used/total（带单位）优先，其次百分比
  const numText = hasBar
    ? `${Math.round(pct)}%`
    : used != null
      ? unit
        ? `${used}${total != null ? `/${total}` : ""} ${unit}`
        : String(used)
      : "";

  return (
    <div>
      <div className="flex items-center justify-between gap-2 mb-0.5">
        <span className="text-[9px] text-slate-600/70">{title}</span>
        <span className="flex items-center gap-1.5 min-w-0">
          {used != null && total != null && (
            <span className="num text-[9px] text-slate-500/80">
              {used}
              {unit ? ` ${unit}` : ""} / {total}
            </span>
          )}
          {numText && (
            <span
              className="num text-[10px] font-semibold shrink-0"
              style={{ color: remainingTextColor(remain) }}
            >
              {numText}
            </span>
          )}
        </span>
      </div>
      {hasBar && <ProgressBar pct={pct / 100} gradient={remainingGradient(remain)} />}
      {/* 重置时间：倒计时 / 具体时间点（两项可同时开，跟随设置页偏好） */}
      {resetsAt != null && resetsAt > now && (
        <div className="num text-[8px] text-slate-500/70 mt-0.5">
          {resetDisplay.countdown &&
            t("common.refreshIn", {
              time: formatCountdownCore(resetsAt - now),
            })}
          {resetDisplay.countdown && resetDisplay.datetime && " · "}
          {resetDisplay.datetime && formatResetStamp(resetsAt)}
        </div>
      )}
    </div>
  );
}

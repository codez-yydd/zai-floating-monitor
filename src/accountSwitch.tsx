import { useCallback, useEffect, useRef, useState } from "react";
import { switchAccount } from "./api";
import { useDataCache } from "./DataCache";
import { useI18n } from "./i18n";

/** 切换目标的最小结构：额度卡条目（AccountQuotaEntry）与账号快照（AccountMeta）均满足 */
export interface SwitchTarget {
  id: string;
  display_name: string;
}

/**
 * 账号切换共享逻辑（QuotaPanel / SummaryTab 额度卡内嵌切换用）。
 * 设置页 AccountsCard 有自己的完整实现（含捕获/重命名），不走这里。
 *
 * 流程：点「切换」→ 确认浮层（退出并重启 ZCode 属重操作）→ switchAccount
 * → 成功后刷新当前账号额度与多账号额度，卡片内短暂绿色提示；
 * ZCode 未能自动重启时提示保留在文案里。失败红字保留至下次操作。
 */
export function useAccountSwitch() {
  const { t } = useI18n();
  const { refreshQuota, refreshAccountQuotas } = useDataCache();
  const [confirming, setConfirming] = useState<SwitchTarget | null>(null);
  const [switching, setSwitching] = useState(false);
  const [notice, setNotice] = useState<
    { kind: "ok" | "err"; text: string } | null
  >(null);
  const noticeTimer = useRef<number | null>(null);

  const showNotice = useCallback((kind: "ok" | "err", text: string) => {
    setNotice({ kind, text });
  }, []);

  // 成功提示 2.5s 自动消失；失败保留到下一次操作
  useEffect(() => {
    if (notice?.kind !== "ok") return;
    noticeTimer.current = window.setTimeout(() => setNotice(null), 2500);
    return () => {
      if (noticeTimer.current != null) window.clearTimeout(noticeTimer.current);
    };
  }, [notice]);

  const request = useCallback((account: SwitchTarget) => {
    setNotice(null);
    setConfirming(account);
  }, []);

  const cancel = useCallback(() => setConfirming(null), []);

  const confirm = useCallback(async () => {
    const target = confirming;
    if (!target) return;
    setSwitching(true);
    setNotice(null);
    try {
      const out = await switchAccount(target.id);
      refreshQuota();
      refreshAccountQuotas();
      showNotice(
        "ok",
        out.zcode_relaunched
          ? t("settings.accountsSwitched", { name: out.switched_to })
          : t("settings.accountsSwitched", { name: out.switched_to }) +
              t("settings.accountsRelaunchFail")
      );
      setConfirming(null);
    } catch (e) {
      showNotice("err", String(e));
      setConfirming(null);
    } finally {
      setSwitching(false);
    }
  }, [confirming, refreshQuota, refreshAccountQuotas, showNotice, t]);

  return {
    /** 待确认的目标账号（非空时渲染确认浮层） */
    confirming,
    request,
    cancel,
    confirm,
    switching,
    notice,
  };
}

/** 切换确认浮层：absolute 遮罩覆盖所在卡片（卡片需 relative），复刻
 *  AccountsCard ConfirmDialog 的视觉，确认键天蓝色（常规操作）。 */
export function SwitchConfirmOverlay({
  account,
  switching,
  onConfirm,
  onCancel,
}: {
  account: SwitchTarget;
  switching: boolean;
  onConfirm: () => void;
  onCancel: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("settings.accountsConfirmTitle")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("settings.accountsConfirmDesc", { name: account.display_name })}
        </p>
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            disabled={switching}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            disabled={switching}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40"
          >
            {switching
              ? t("settings.accountsSwitching")
              : t("settings.accountsSwitch")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 卡片内嵌的小切换按钮（非当前账号行用） */
export function SwitchAccountButton({
  onClick,
  disabled,
}: {
  onClick: () => void;
  disabled?: boolean;
}) {
  const { t } = useI18n();
  return (
    <button
      onClick={onClick}
      disabled={disabled}
      className="shrink-0 text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
    >
      {t("settings.accountsSwitch")}
    </button>
  );
}

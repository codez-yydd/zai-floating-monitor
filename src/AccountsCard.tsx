import { useCallback, useEffect, useMemo, useState } from "react";
import type { AccountMeta, AccountQuotaEntry, AccountsState } from "./types";
import {
  captureAccount,
  listAccounts,
  removeAccount,
  renameAccount,
  switchAccount,
} from "./api";
import { useDataCache } from "./DataCache";
import { levelLabel } from "./format";
import { remainingTextColor } from "./widgets";
import { SettingsCard } from "./layout";
import { useI18n } from "./i18n";

/**
 * 多智谱账号切换卡片（设置页，挂在统计来源卡与汇率卡之间）。
 * - 捕获：把当前 ZCode 客户端登录态存为快照（仅存本机 ~/.zbar/accounts/，
 *   目录 0700 / 文件 0600，不参与同步）
 * - 切换：退出 ZCode → 原文写回凭证 → 重启；失败自动回滚（后端事务保证）
 * 列表数据不进 DataCache（配置类数据，组件自管 state）。
 */
export function AccountsCard() {
  const { t } = useI18n();
  const { refreshQuota, accountQuotas, refreshAccountQuotas } = useDataCache();
  const [state, setState] = useState<AccountsState | null>(null);
  const [loadError, setLoadError] = useState<string | null>(null);
  // 捕获/切换进行中（防重复触发；切换含进程轮询可达数秒）
  const [capturing, setCapturing] = useState(false);
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  // 操作结果反馈（成功 flash 短暂展示；失败文案保留至下次操作）
  const [flash, setFlash] = useState<string | null>(null);
  const [actionError, setActionError] = useState<string | null>(null);
  // 弹窗：切换确认 / 删除确认 / 重命名
  const [confirmSwitch, setConfirmSwitch] = useState<AccountMeta | null>(null);
  const [confirmRemove, setConfirmRemove] = useState<AccountMeta | null>(null);
  const [renaming, setRenaming] = useState<AccountMeta | null>(null);

  const showFlash = useCallback((text: string) => {
    setFlash(text);
    setTimeout(() => setFlash(null), 2000);
  }, []);

  const reload = useCallback(() => {
    listAccounts()
      .then((s) => {
        setState(s);
        setLoadError(null);
      })
      .catch((e) =>
        setLoadError(t("settings.accountsLoadFail", { msg: String(e) }))
      );
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  useEffect(() => {
    reload();
  }, [reload]);

  // 捕获当前登录（同账号重复捕获为更新）
  const handleCapture = async () => {
    setCapturing(true);
    setActionError(null);
    try {
      const out = await captureAccount();
      showFlash(
        out.updated_existing
          ? t("settings.accountsCapturedUpdate")
          : t("settings.accountsCapturedNew")
      );
      reload();
      refreshAccountQuotas();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setCapturing(false);
    }
  };

  // 切换账号（弹窗确认后执行；成功后刷新额度数据）
  const handleSwitch = async (account: AccountMeta) => {
    setSwitchingId(account.id);
    setActionError(null);
    try {
      const out = await switchAccount(account.id);
      showFlash(
        out.zcode_relaunched
          ? t("settings.accountsSwitched", { name: out.switched_to })
          : t("settings.accountsSwitched", { name: out.switched_to }) +
            t("settings.accountsRelaunchFail")
      );
      refreshQuota();
      reload();
      refreshAccountQuotas();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setSwitchingId(null);
      setConfirmSwitch(null);
    }
  };

  const handleRemove = async (account: AccountMeta) => {
    setActionError(null);
    try {
      await removeAccount(account.id);
      reload();
      refreshAccountQuotas();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setConfirmRemove(null);
    }
  };

  const handleRename = async (id: string, name: string) => {
    setActionError(null);
    try {
      await renameAccount(id, name);
      reload();
      refreshAccountQuotas();
    } catch (e) {
      setActionError(String(e));
    } finally {
      setRenaming(null);
    }
  };

  const busy = capturing || switchingId !== null;
  const current = state?.current ?? null;
  // 当前账号行：匹配到快照时显示其名称，否则显示未识别
  const currentName =
    state?.accounts.find((a) => a.id === current?.matched_snapshot_id)
      ?.display_name ?? null;
  // 各账号额度（DataCache 低频刷新 + 账号操作后手动刷），按 id 关联到列表行
  const quotaMap = useMemo(
    () => new Map(accountQuotas.map((e) => [e.id, e])),
    [accountQuotas]
  );

  return (
    <div className="relative">
      <SettingsCard
        title={t("settings.accountsCard")}
        action={
          <button
            onClick={handleCapture}
            disabled={busy}
            className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-50 disabled:cursor-not-allowed"
          >
            {capturing
              ? t("settings.accountsCapturing")
              : t("settings.accountsCapture")}
          </button>
        }
        hint={t("settings.accountsHint")}
      >
        {/* 当前登录行：匹配快照名 / 未识别，副文本邮箱或指纹前 8 位 */}
        <div className="flex items-center justify-between gap-2 rounded-md px-1.5 py-1 bg-slate-900/5">
          <span className="flex items-center gap-1.5 min-w-0">
            <span className="text-[10px] text-slate-700/55 shrink-0">
              {t("settings.accountsCurrent")}
            </span>
            <span className="text-[10px] text-slate-900/85 truncate">
              {current
                ? (currentName ?? t("settings.accountsUnknown"))
                : t("settings.accountsUnknown")}
            </span>
          </span>
          {current && (
            <span className="num text-[9px] text-slate-700/45 shrink-0">
              {current.email ?? `#${current.fingerprint.slice(0, 8)}`}
            </span>
          )}
        </div>

        {/* 空态引导 */}
        {state && state.accounts.length === 0 && (
          <p className="text-[9px] text-slate-500 leading-relaxed mt-1.5">
            {t("settings.accountsEmpty")}
          </p>
        )}

        {/* 快照列表 */}
        <div className="mt-1.5 space-y-0.5">
          {state?.accounts.map((a) => {
            const q = quotaMap.get(a.id);
            return (
              <div
                key={a.id}
                className="flex items-center justify-between gap-2 rounded-md px-1.5 py-1 hover:bg-slate-900/5 transition-colors"
              >
                <span className="min-w-0">
                  <span className="flex items-center gap-1.5">
                    <span className="text-[10px] text-slate-900/80 truncate">
                      {a.display_name}
                    </span>
                    {a.is_current && (
                      <span className="shrink-0 text-[8px] px-1 py-px rounded-full bg-violet-500/10 text-violet-600">
                        {t("settings.accountsCurrent")}
                      </span>
                    )}
                  </span>
                  <span className="block text-[9px] text-slate-700/45 truncate">
                    {a.email ?? `#${a.fingerprint.slice(0, 8)}`}
                  </span>
                  <AccountQuotaLine entry={q} />
                </span>
                <span className="flex items-center gap-1 shrink-0">
                  <button
                    onClick={() => setConfirmSwitch(a)}
                    disabled={busy || a.is_current}
                    title={
                      a.is_current ? t("settings.accountsCurrent") : undefined
                    }
                    className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {switchingId === a.id
                      ? t("settings.accountsSwitching")
                      : t("settings.accountsSwitch")}
                  </button>
                  <button
                    onClick={() => setRenaming(a)}
                    disabled={busy}
                    className="text-[9px] px-1.5 py-0.5 rounded bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {t("settings.accountsRename")}
                  </button>
                  <button
                    onClick={() => setConfirmRemove(a)}
                    disabled={busy}
                    className="text-[9px] px-1.5 py-0.5 rounded bg-rose-500/10 text-rose-600/90 hover:bg-rose-500/20 transition-colors disabled:opacity-40 disabled:cursor-not-allowed"
                  >
                    {t("common.delete")}
                  </button>
                </span>
              </div>
            );
          })}
        </div>

        {/* 读取失败 */}
        {loadError && (
          <p className="text-[9px] text-rose-600 mt-1.5 leading-relaxed break-all">
            {loadError}
          </p>
        )}
        {/* 操作反馈：成功 flash / 失败保留 */}
        {flash && (
          <p className="text-[9px] text-emerald-600 mt-1.5 leading-relaxed break-all">
            {flash}
          </p>
        )}
        {actionError && (
          <p className="text-[9px] text-rose-600 mt-1.5 leading-relaxed break-all">
            {actionError}
          </p>
        )}
      </SettingsCard>

      {/* 切换确认弹窗（本地复刻 SyncPanel ConfirmDialog 样式，天蓝确认键） */}
      {confirmSwitch && !busy && (
        <ConfirmDialog
          title={t("settings.accountsConfirmTitle")}
          desc={t("settings.accountsConfirmDesc", {
            name: confirmSwitch.display_name,
          })}
          confirmText={t("settings.accountsSwitch")}
          danger={false}
          onCancel={() => setConfirmSwitch(null)}
          onConfirm={() => handleSwitch(confirmSwitch)}
        />
      )}

      {/* 删除确认弹窗（红色键） */}
      {confirmRemove && !busy && (
        <ConfirmDialog
          title={`${t("common.delete")} · ${confirmRemove.display_name}`}
          desc={t("settings.accountsRemoveConfirm")}
          confirmText={t("common.delete")}
          danger={true}
          onCancel={() => setConfirmRemove(null)}
          onConfirm={() => handleRemove(confirmRemove)}
        />
      )}

      {/* 重命名弹窗（本地复刻 SyncPanel RenameDialog 样式） */}
      {renaming && !busy && (
        <RenameDialog
          name={renaming.display_name}
          onCancel={() => setRenaming(null)}
          onConfirm={(name) => handleRename(renaming.id, name)}
        />
      )}
    </div>
  );
}

/** 账号行额度副行：等级徽标（sky，与汇总页 ZCode 徽标同色）+ 每周剩余（着色）
 *  + 5小时剩余小字。快照尚未查到额度（entry=undefined）时不渲染；查询失败显示
 *  ⚠ 失败文案（title 放原因）；周数据缺失时只显示 5h，不冒充"剩余 100%"。 */
function AccountQuotaLine({ entry }: { entry?: AccountQuotaEntry }) {
  const { t } = useI18n();
  if (!entry) return null;

  if (!entry.quota) {
    return (
      <span
        className="block text-[9px] text-rose-500/90 truncate"
        title={entry.error ?? undefined}
      >
        ⚠ {t("quota.quotaFail")}
      </span>
    );
  }

  const q = entry.quota;
  const weeklyRemain = q.weekly ? Math.max(0, 100 - q.weekly.percentage) : null;
  const hour5Remain = q.hour5 ? Math.max(0, 100 - q.hour5.percentage) : null;

  return (
    <span className="flex items-center gap-1.5 mt-0.5">
      {q.level && (
        <span className="shrink-0 px-1 py-px rounded text-[8px] font-semibold bg-sky-500/12 text-sky-700">
          {levelLabel(q.level)}
        </span>
      )}
      {weeklyRemain != null && (
        <span
          className="num text-[9px] font-medium"
          style={{ color: remainingTextColor(weeklyRemain) }}
        >
          {t("quota.weekShort")} {Math.round(weeklyRemain)}%
        </span>
      )}
      {hour5Remain != null && (
        <span className="num text-[9px] text-slate-700/45">
          {t("quota.hour5Short")} {Math.round(hour5Remain)}%
        </span>
      )}
    </span>
  );
}

/** 通用确认弹窗（danger 切换确认/删除确认两种配色） */
function ConfirmDialog({
  title,
  desc,
  confirmText,
  danger,
  onCancel,
  onConfirm,
}: {
  title: string;
  desc: string;
  confirmText: string;
  danger: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {title}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {desc}
        </p>
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            className={`flex-1 text-[11px] py-1 rounded-md text-white transition-colors ${
              danger
                ? "bg-red-500 hover:bg-red-600"
                : "bg-sky-500 hover:bg-sky-600"
            }`}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 重命名弹窗：input + 32 字上限 + Enter 确认 */
function RenameDialog({
  name,
  onCancel,
  onConfirm,
}: {
  name: string;
  onCancel: () => void;
  onConfirm: (newName: string) => void;
}) {
  const { t } = useI18n();
  const [draft, setDraft] = useState(name);
  const trimmed = draft.trim();
  const valid = trimmed.length > 0 && trimmed.length <= 32;

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("settings.accountsRename")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("settings.accountsRenameDesc")}
        </p>
        <input
          type="text"
          value={draft}
          maxLength={32}
          autoFocus
          onChange={(e) => setDraft(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter" && valid) onConfirm(trimmed);
          }}
          className="w-full mb-1 px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
        />
        <div className="text-[9px] text-slate-700/40 mb-2 text-right">
          {trimmed.length}/32
        </div>
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("common.cancel")}
          </button>
          <button
            disabled={!valid}
            onClick={() => onConfirm(trimmed)}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40"
          >
            {t("common.confirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

import { useEffect, useState } from "react";
import type {
  CleanupResult,
  CleanupStatus,
  DeviceInfo,
  RegisterRequest,
  SyncConfig,
  SyncMode,
} from "./types";
import {
  cleanupServer,
  disconnectDevice,
  getCleanupStatus,
  getSyncConfig,
  mergeDevices,
  pendingUploadCount,
  registerDevice,
  renameDevice,
  setAutoCleanup,
  setSyncConfig,
  syncNow,
} from "./api";
import {
  PageShell,
  PageHeader,
  PageBody,
  SettingsCard,
  BtnPrimary,
  AlertBanner,
  LoadingState,
} from "./layout";
import { useI18n, type TFn } from "./i18n";

interface Props {
  onBack: () => void;
}

/** 相对当前时间的友好描述（文案走词典） */
function timeAgo(t: TFn, ms: number): string {
  if (!ms) return t("sync.never");
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return t("sync.justNow");
  if (min < 60) return t("sync.minAgo", { n: min });
  const hr = Math.floor(min / 60);
  if (hr < 24) return t("sync.hourAgo", { n: hr });
  return t("sync.dayAgo", { n: Math.floor(hr / 24) });
}

export function SyncPanel({ onBack }: Props) {
  const { t } = useI18n();
  const [config, setConfig] = useState<SyncConfig | null>(null);
  const [loading, setLoading] = useState(true);
  const [error, setError] = useState<string | null>(null);

  // 注册表单
  const [regForm, setRegForm] = useState<RegisterRequest>({
    server_url: "",
    master_token: "",
    device_name: "",
  });
  const [registering, setRegistering] = useState(false);
  const [showMaster, setShowMaster] = useState(false);

  // 同步状态
  const [pending, setPending] = useState<number>(0);
  const [syncing, setSyncing] = useState(false);
  const [syncFlash, setSyncFlash] = useState<string | null>(null);

  // 模式编辑
  const [modeDraft, setModeDraft] = useState<SyncMode>("manual");
  const [intervalDraft, setIntervalDraft] = useState(60);
  // 自动清理保留天数草稿（onBlur 提交，避免每键发请求）
  const [daysDraft, setDaysDraft] = useState(30);

  // 数据管理
  const [cleanupStatus, setCleanupStatus] = useState<CleanupStatus | null>(null);
  const [masterInput, setMasterInput] = useState("");
  const [confirmAction, setConfirmAction] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);

  // 设备合并 / 改名
  const [mergeSource, setMergeSource] = useState<DeviceInfo | null>(null);
  const [renameTarget, setRenameTarget] = useState<DeviceInfo | null>(null);

  const refresh = async (cfg: SyncConfig) => {
    setConfig(cfg);
    setModeDraft(cfg.mode);
    setIntervalDraft(cfg.interval_seconds);
    try {
      setPending(await pendingUploadCount());
    } catch {
      /* 本地无库时忽略 */
    }
    if (cfg.enabled) {
      try {
        const st = await getCleanupStatus();
        setCleanupStatus(st);
        setDaysDraft(st.auto_config.auto_days || 30);
      } catch {
        /* 服务器不可达时忽略 */
      }
    }
  };

  useEffect(() => {
    (async () => {
      setLoading(true);
      try {
        const cfg = await getSyncConfig();
        await refresh(cfg);
      } catch (e) {
        setError(String(e));
      } finally {
        setLoading(false);
      }
    })();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleRegister = async () => {
    setRegistering(true);
    setError(null);
    try {
      const cfg = await registerDevice(regForm);
      await refresh(cfg);
      setRegForm({ server_url: "", master_token: "", device_name: "" });
    } catch (e) {
      setError(String(e));
    } finally {
      setRegistering(false);
    }
  };

  const handleSaveMode = async () => {
    if (!config) return;
    const next = { ...config, mode: modeDraft, interval_seconds: intervalDraft };
    setError(null);
    try {
      await setSyncConfig(next);
      setConfig(next);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleSyncNow = async () => {
    setSyncing(true);
    setError(null);
    try {
      const outcome = await syncNow();
      setSyncFlash(t("sync.uploaded", { count: outcome.uploaded }));
      setTimeout(() => setSyncFlash(null), 2000);
      const cfg = await getSyncConfig();
      await refresh(cfg);
    } catch (e) {
      setError(String(e));
    } finally {
      setSyncing(false);
    }
  };

  const handleDisconnect = async () => {
    setError(null);
    try {
      await disconnectDevice();
      const cfg = await getSyncConfig();
      await refresh(cfg);
    } catch (e) {
      setError(String(e));
    }
  };

  const handleCleanup = async (
    action: "device" | "before" | "all" | "reset",
    deviceId?: string,
    days?: number
  ) => {
    if (!masterInput.trim()) {
      setError(t("sync.needMaster"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res: CleanupResult = await cleanupServer(
        masterInput.trim(),
        action,
        deviceId,
        days
      );
      const msg =
        res.devices_deleted != null
          ? t("sync.deletedBoth", {
              records: res.records_deleted,
              devices: res.devices_deleted,
            })
          : t("sync.deleted", { count: res.records_deleted });
      setSyncFlash(msg);
      setTimeout(() => setSyncFlash(null), 2500);
      setConfirmAction(null);
      // 刷新状态
      const cfg = await getSyncConfig();
      await refresh(cfg);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleMerge = async (sourceId: string, targetId: string) => {
    if (!masterInput.trim()) {
      setError(t("sync.needMaster"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await mergeDevices(masterInput.trim(), sourceId, targetId);
      const tName =
        cleanupStatus?.devices.find((d) => d.device_id === targetId)
          ?.device_name ?? t("sync.targetDevice");
      setSyncFlash(t("sync.merged", { count: res.records_moved, name: tName }));
      setTimeout(() => setSyncFlash(null), 2500);
      setMergeSource(null);
      const cfg = await getSyncConfig();
      await refresh(cfg);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleRename = async (deviceId: string, newName: string) => {
    if (!masterInput.trim()) {
      setError(t("sync.needMaster"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await renameDevice(masterInput.trim(), deviceId, newName);
      if (res.updated === 0) {
        setError(t("sync.deviceMissing"));
        setRenameTarget(null);
        const cfg = await getSyncConfig();
        await refresh(cfg);
        return;
      }
      // 若改的是本机设备名，回写 sync.json 保持一致，避免下次同步注册成新名字。
      // 保存前重新拉最新配置、只覆盖 device_name：组件内的 config 是打开面板时的
      // 快照，直接全量写回会把后台同步线程刚推进的 last_uploaded_rowid 回退，
      // 造成一批记录重复上传
      if (config && config.device_id === deviceId) {
        const latest = await getSyncConfig();
        await setSyncConfig({ ...latest, device_name: newName });
      }
      setSyncFlash(t("sync.renamed", { name: newName }));
      setTimeout(() => setSyncFlash(null), 2500);
      setRenameTarget(null);
      const cfg = await getSyncConfig();
      await refresh(cfg);
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  const handleAutoCleanup = async (enabled: boolean, days: number) => {
    if (!masterInput.trim()) {
      setError(t("sync.needMasterAuto"));
      return;
    }
    setBusy(true);
    setError(null);
    try {
      await setAutoCleanup(masterInput.trim(), enabled, days);
      if (config?.enabled) {
        setCleanupStatus(await getCleanupStatus());
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setBusy(false);
    }
  };

  if (loading) return <LoadingState text={t("sync.loading")} />;

  const connected = config?.enabled && config.device_token;

  return (
    <PageShell>
      <PageHeader title={t("sync.title")} onBack={onBack} />
      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}

        {!connected ? (
          <SettingsCard title={t("sync.connectTitle")} hint={t("sync.connectHint")}>

            <label className="flex flex-col gap-0.5 text-[10px]">
              <span className="text-slate-700/55">{t("sync.serverUrl")}</span>
              <input
                type="text"
                value={regForm.server_url}
                placeholder="http://192.168.1.100:3838"
                onChange={(e) =>
                  setRegForm((f) => ({ ...f, server_url: e.target.value }))
                }
                className="px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none focus:border-sky-400/60"
              />
            </label>

            <label className="flex flex-col gap-0.5 text-[10px]">
              <span className="text-slate-700/55">{t("sync.masterLabel")}</span>
              <div className="flex items-center rounded-md bg-slate-900/5 border border-slate-900/10 focus-within:border-sky-400/60">
                <input
                  type={showMaster ? "text" : "password"}
                  value={regForm.master_token}
                  placeholder={t("sync.masterPh")}
                  onChange={(e) =>
                    setRegForm((f) => ({ ...f, master_token: e.target.value }))
                  }
                  className="num w-full px-1.5 py-1 bg-transparent text-[11px] text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none"
                />
                <button
                  onClick={() => setShowMaster((v) => !v)}
                  className="px-1.5 text-slate-700/40 hover:text-slate-900/70 text-[10px] shrink-0"
                >
                  {showMaster ? "🙈" : "👁"}
                </button>
              </div>
            </label>

            <label className="flex flex-col gap-0.5 text-[10px]">
              <span className="text-slate-700/55">{t("sync.deviceName")}</span>
              <input
                type="text"
                value={regForm.device_name}
                placeholder={t("sync.namePh")}
                onChange={(e) =>
                  setRegForm((f) => ({ ...f, device_name: e.target.value }))
                }
                className="px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none focus:border-sky-400/60"
              />
            </label>

            {regForm.server_url.startsWith("http://") && (
              <p className="text-[9px] text-amber-600/80 leading-relaxed">
                {t("sync.httpWarn")}
              </p>
            )}

            <BtnPrimary
              onClick={handleRegister}
              disabled={registering || !regForm.server_url.trim() || !regForm.master_token.trim() || !regForm.device_name.trim()}
              className="w-full mt-2"
            >
              {registering ? t("sync.connecting") : t("sync.connect")}
            </BtnPrimary>
          </SettingsCard>
        ) : (
          <>
            <SettingsCard title={config!.device_name} action={
              <span className="text-[9px] text-slate-500 font-mono">{config!.device_id.slice(0, 8)}</span>
            }>
              <div className="mt-1.5 grid grid-cols-2 gap-1.5 text-[10px]">
                <div className="text-slate-700/55">
                  {t("sync.server")}
                  <div className="text-slate-900/80 truncate">
                    {config!.server_url}
                  </div>
                </div>
                <div className="text-slate-700/55">
                  {t("sync.lastSync")}
                  <div className="text-slate-900/80">
                    {timeAgo(t, config!.last_sync_at)}
                  </div>
                </div>
                <div className="text-slate-700/55">
                  {t("sync.pending")}
                  <div className="text-slate-900/80">
                    {t("sync.recordsCount", { count: pending })}
                  </div>
                </div>
                <div className="text-slate-700/55">
                  {t("sync.uploadCursor")}
                  <div className="text-slate-900/80">
                    #{config!.last_uploaded_rowid}
                  </div>
                </div>
              </div>

              {/* 同步模式 */}
              <div className="mt-2.5 pt-2 border-t border-slate-900/10">
                <div className="flex items-center justify-between">
                  <span className="text-[10px] text-slate-700/55">{t("sync.mode")}</span>
                  <div className="flex gap-1">
                    {(["manual", "auto"] as SyncMode[]).map((m) => (
                      <button
                        key={m}
                        onClick={() => setModeDraft(m)}
                        className={`px-2 py-0.5 rounded-md text-[10px] transition-colors ${
                          modeDraft === m
                            ? "bg-sky-500 text-white"
                            : "bg-slate-900/5 text-slate-700/65 hover:bg-slate-900/10"
                        }`}
                      >
                        {m === "manual" ? t("sync.manual") : t("sync.auto")}
                      </button>
                    ))}
                  </div>
                </div>
                {modeDraft === "auto" && (
                  <label className="flex items-center justify-between mt-1.5 text-[10px]">
                    <span className="text-slate-700/55">{t("sync.interval")}</span>
                    <input
                      type="number"
                      min={10}
                      value={intervalDraft}
                      onChange={(e) =>
                        setIntervalDraft(
                          Math.max(10, parseInt(e.target.value) || 60)
                        )
                      }
                      className="w-16 px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-right text-[11px] focus:outline-none focus:border-sky-400/60"
                    />
                  </label>
                )}
                <div className="flex gap-1.5 mt-2">
                  <button
                    onClick={handleSyncNow}
                    disabled={syncing}
                    className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
                  >
                    {syncing
                      ? t("sync.syncing")
                      : syncFlash
                        ? syncFlash
                        : t("sync.syncNow")}
                  </button>
                  <button
                    onClick={handleSaveMode}
                    className="text-[11px] px-2.5 py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
                  >
                    {t("sync.saveMode")}
                  </button>
                  <button
                    onClick={handleDisconnect}
                    className="text-[11px] px-2.5 py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                  >
                    {t("sync.disconnect")}
                  </button>
                </div>
              </div>
            </SettingsCard>

            <SettingsCard
              title={t("sync.dataMgmt")}
              action={cleanupStatus ? <span className="text-[10px] text-slate-500">{t("sync.totalRecords", { count: cleanupStatus.total_records })}</span> : undefined}
            >
              <label className="flex flex-col gap-0.5 text-[10px] mb-2">
                <span className="text-slate-600">{t("sync.masterForCleanup")}</span>
                <input
                  type="password"
                  value={masterInput}
                  placeholder={t("sync.pasteMasterPh")}
                  onChange={(e) => setMasterInput(e.target.value)}
                  className="input-box"
                />
              </label>

              {/* 自动清理配置 */}
              <div className="flex items-center justify-between mb-1.5 py-1 border-t border-slate-900/10">
                <span className="text-[10px] text-slate-700/55">
                  {t("sync.autoCleanup")}
                </span>
                <button
                  onClick={() =>
                    handleAutoCleanup(
                      !cleanupStatus?.auto_config.auto_enabled,
                      cleanupStatus?.auto_config.auto_days || 30
                    )
                  }
                  disabled={busy}
                  className={`px-2 py-0.5 rounded-md text-[10px] transition-colors ${
                    cleanupStatus?.auto_config.auto_enabled
                      ? "bg-emerald-500 text-white"
                      : "bg-slate-900/10 text-slate-700/65"
                  }`}
                >
                  {cleanupStatus?.auto_config.auto_enabled ? t("sync.on") : t("sync.off")}
                </button>
              </div>
              {cleanupStatus?.auto_config.auto_enabled && (
                <label className="flex items-center justify-between mb-2 text-[10px]">
                  <span className="text-slate-700/55">{t("sync.keepDays")}</span>
                  <input
                    type="number"
                    min={1}
                    value={daysDraft}
                    onChange={(e) => setDaysDraft(parseInt(e.target.value) || 30)}
                    onBlur={() => {
                      const days = Math.max(1, daysDraft);
                      if (days !== cleanupStatus.auto_config.auto_days) {
                        handleAutoCleanup(true, days);
                      }
                    }}
                    className="w-16 px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-right text-[11px] focus:outline-none focus:border-sky-400/60"
                  />
                </label>
              )}

              {/* 各设备记录数 */}
              {cleanupStatus && cleanupStatus.devices.length > 0 && (
                <div className="space-y-1 mb-2 border-t border-slate-900/10 pt-1.5">
                  {cleanupStatus.devices.map((d: DeviceInfo) => (
                    <div
                      key={d.device_id}
                      className="flex items-center justify-between text-[10px]"
                    >
                      <span className="text-slate-700/70 min-w-0 truncate">
                        {d.device_name}
                        <span className="text-slate-700/40 ml-1 font-mono">
                          {d.device_id.slice(0, 6)}
                        </span>
                        {d.device_id === config!.device_id && (
                          <span className="ml-1 text-sky-600/70">{t("sync.localBadge")}</span>
                        )}
                      </span>
                      <span className="flex items-center gap-1.5 shrink-0">
                        <span className="text-slate-700/45">
                          {t("sync.recordsCount", { count: d.record_count ?? 0 })}
                        </span>
                        <button
                          onClick={() => setRenameTarget(d)}
                          disabled={busy}
                          className="text-slate-700/40 hover:text-sky-600 transition-colors"
                          title={t("sync.rename")}
                        >
                          ✎
                        </button>
                        {d.device_id !== config!.device_id && (
                          <>
                            <button
                              onClick={() => setMergeSource(d)}
                              disabled={busy}
                              className="text-slate-700/50 hover:text-sky-600 transition-colors"
                              title={t("sync.mergeInto")}
                            >
                              {t("sync.merge")}
                            </button>
                            <button
                              onClick={() =>
                                setConfirmAction(`device:${d.device_id}`)
                              }
                              disabled={busy}
                              className="text-slate-700/40 hover:text-red-600 transition-colors"
                              title={t("sync.deleteDeviceData")}
                            >
                              ✕
                            </button>
                          </>
                        )}
                      </span>
                    </div>
                  ))}
                </div>
              )}

              {/* 危险操作 */}
              <div className="flex gap-1.5 pt-1.5 border-t border-slate-900/10">
                <button
                  onClick={() => setConfirmAction("before")}
                  disabled={busy}
                  className="flex-1 text-[10px] py-1 rounded-md bg-slate-900/5 text-slate-700/65 hover:bg-amber-500/15 hover:text-amber-700 transition-colors"
                >
                  {t("sync.cleanByTime")}
                </button>
                <button
                  onClick={() => setConfirmAction("all")}
                  disabled={busy}
                  className="flex-1 text-[10px] py-1 rounded-md bg-slate-900/5 text-slate-700/65 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                >
                  {t("sync.clearAll")}
                </button>
                <button
                  onClick={() => setConfirmAction("reset")}
                  disabled={busy}
                  className="flex-1 text-[10px] py-1 rounded-md bg-slate-900/5 text-slate-700/65 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                >
                  {t("sync.reset")}
                </button>
              </div>
            </SettingsCard>
          </>
        )}
      </PageBody>

      {/* 确认弹层 */}
      {confirmAction && (
        <ConfirmDialog
          action={confirmAction}
          onCancel={() => setConfirmAction(null)}
          onConfirm={(action, deviceId, days) => {
            const [a, id] = action.split(":");
            handleCleanup(
              a as "device" | "before" | "all" | "reset",
              id || deviceId,
              days
            );
          }}
        />
      )}

      {/* 设备合并弹层 */}
      {mergeSource && cleanupStatus && (
        <MergeDialog
          source={mergeSource}
          devices={cleanupStatus.devices}
          localDeviceId={config?.device_id ?? ""}
          onCancel={() => setMergeSource(null)}
          onConfirm={(targetId) =>
            handleMerge(mergeSource.device_id, targetId)
          }
        />
      )}

      {/* 设备改名弹层 */}
      {renameTarget && (
        <RenameDialog
          device={renameTarget}
          onCancel={() => setRenameTarget(null)}
          onConfirm={(newName) =>
            handleRename(renameTarget.device_id, newName)
          }
        />
      )}
    </PageShell>
  );
}

/** 设备合并对话框：选目标设备，把来源并入目标后删除来源 */
function MergeDialog({
  source,
  devices,
  localDeviceId,
  onCancel,
  onConfirm,
}: {
  source: DeviceInfo;
  devices: DeviceInfo[];
  localDeviceId: string;
  onCancel: () => void;
  onConfirm: (targetId: string) => void;
}) {
  const { t } = useI18n();
  // 候选目标 = 除来源外的全部设备。默认选最新注册的那个（通常是该设备当前正在用的
  // 实例），而不是本机：合并到本机会让历史数据在本机"全部汇总"视图中不可见（本机
  // 读本地库、远端查询又排除本机），故只在用户明确选择本机时才走这条路径。
  const candidates = devices.filter((d) => d.device_id !== source.device_id);
  const defaultTarget =
    [...candidates].sort((a, b) => b.created_at - a.created_at)[0]?.device_id ??
    candidates[0]?.device_id ??
    "";
  const [target, setTarget] = useState<string>(defaultTarget);
  const targetIsLocal = target === localDeviceId;

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("sync.mergeTitle")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("sync.mergeDesc", {
            name: source.device_name,
            id: source.device_id.slice(0, 6),
            count: source.record_count ?? 0,
          })}
        </p>
        <select
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          className="w-full mb-2 px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
        >
          {candidates.map((d) => (
            <option key={d.device_id} value={d.device_id}>
              {t("common.deviceOption", {
                name: d.device_name,
                id: d.device_id.slice(0, 6),
              })}
              {d.device_id === localDeviceId ? ` · ${t("sync.localBadge")}` : ""}
            </option>
          ))}
        </select>
        {targetIsLocal ? (
          <p className="text-[10px] text-amber-700/80 leading-relaxed mb-2">
            {t("sync.mergeLocalWarn")}
          </p>
        ) : (
          <p className="text-[10px] text-amber-700/80 leading-relaxed mb-2">
            {t("sync.mergeWarn")}
          </p>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("common.cancel")}
          </button>
          <button
            disabled={!target}
            onClick={() => onConfirm(target)}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40"
          >
            {t("sync.mergeConfirm")}
          </button>
        </div>
      </div>
    </div>
  );
}

/** 设备改名对话框 */
function RenameDialog({
  device,
  onCancel,
  onConfirm,
}: {
  device: DeviceInfo;
  onCancel: () => void;
  onConfirm: (newName: string) => void;
}) {
  const { t } = useI18n();
  const [name, setName] = useState(device.device_name);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= 32;

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {t("sync.renameTitle")}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {t("sync.renameDesc", {
            name: device.device_name,
            id: device.device_id.slice(0, 6),
          })}
        </p>
        <input
          type="text"
          value={name}
          maxLength={32}
          autoFocus
          onChange={(e) => setName(e.target.value)}
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

/** 清理确认对话框（按 action 类型显示不同提示 + 参数输入） */
function ConfirmDialog({
  action,
  onCancel,
  onConfirm,
}: {
  action: string;
  onCancel: () => void;
  onConfirm: (
    action: string,
    deviceId?: string,
    days?: number
  ) => void;
}) {
  const { t } = useI18n();
  const [type, arg] = [action.split(":")[0], action.split(":")[1]];
  const [days, setDays] = useState(30);

  const title =
    type === "device"
      ? t("sync.confirmTitleDevice")
      : type === "before"
        ? t("sync.cleanByTime")
        : type === "all"
          ? t("sync.clearAll")
          : t("sync.confirmTitleReset");

  const desc =
    type === "device"
      ? t("sync.confirmDescDevice")
      : type === "before"
        ? t("sync.confirmDescBefore", { days })
        : type === "all"
          ? t("sync.confirmDescAll")
          : t("sync.confirmDescReset");

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {title}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          {desc}
        </p>
        {type === "before" && (
          <label className="flex items-center justify-between text-[10px] mb-2">
            <span className="text-slate-700/55">{t("sync.keepDays")}</span>
            <input
              type="number"
              min={1}
              value={days}
              onChange={(e) =>
                setDays(Math.max(1, parseInt(e.target.value) || 30))
              }
              className="w-16 px-1.5 py-0.5 rounded-md bg-slate-900/5 border border-slate-900/10 text-right text-[11px] focus:outline-none"
            />
          </label>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={() =>
              onConfirm(
                type === "device" ? `device:${arg}` : type,
                type === "device" ? arg : undefined,
                type === "before" ? days : undefined
              )
            }
            className="flex-1 text-[11px] py-1 rounded-md bg-red-500 text-white hover:bg-red-600 transition-colors"
          >
            {t("sync.confirmDelete")}
          </button>
        </div>
      </div>
    </div>
  );
}

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

interface Props {
  onBack: () => void;
}

/** 相对当前时间的友好描述 */
function timeAgo(ms: number): string {
  if (!ms) return "从未";
  const diff = Date.now() - ms;
  const min = Math.floor(diff / 60000);
  if (min < 1) return "刚刚";
  if (min < 60) return `${min} 分钟前`;
  const hr = Math.floor(min / 60);
  if (hr < 24) return `${hr} 小时前`;
  return `${Math.floor(hr / 24)} 天前`;
}

export function SyncPanel({ onBack }: Props) {
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
      setSyncFlash(`已上传 ${outcome.uploaded} 条`);
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
      setError("请先填写 Master Token（从服务器日志获取）");
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
          ? `已删除 ${res.records_deleted} 条记录、${res.devices_deleted} 个设备`
          : `已删除 ${res.records_deleted} 条记录`;
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
      setError("请先填写 Master Token（从服务器日志获取）");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await mergeDevices(masterInput.trim(), sourceId, targetId);
      const tName =
        cleanupStatus?.devices.find((d) => d.device_id === targetId)
          ?.device_name ?? "目标设备";
      setSyncFlash(`已合并 ${res.records_moved} 条记录到「${tName}」`);
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
      setError("请先填写 Master Token（从服务器日志获取）");
      return;
    }
    setBusy(true);
    setError(null);
    try {
      const res = await renameDevice(masterInput.trim(), deviceId, newName);
      if (res.updated === 0) {
        setError("设备不存在或已被删除");
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
      setSyncFlash(`已改名为「${newName}」`);
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
      setError("配置自动清理需要 Master Token");
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

  if (loading) {
    return (
      <div className="flex items-center justify-center h-full text-xs text-slate-700/55">
        加载中…
      </div>
    );
  }

  const connected = config?.enabled && config.device_token;

  return (
    <div className="flex flex-col h-full">
      {/* 顶部 */}
      <div className="px-3.5 py-2.5 border-b border-slate-900/10">
        <div className="flex items-center justify-between mb-1">
          <button
            onClick={onBack}
            className="text-xs text-slate-700/60 hover:text-sky-600 transition-colors"
          >
            ← 返回
          </button>
          <h1 className="text-[13px] font-semibold text-slate-900/90">
            设备同步
          </h1>
          <span className="w-8" />
        </div>
      </div>

      <div className="flex-1 overflow-y-auto px-3.5 py-2.5 space-y-2.5">
        {error && (
          <div className="px-2.5 py-1.5 rounded-lg bg-red-500/15 text-red-700 text-xs">
            {error}
          </div>
        )}

        {!connected ? (
          /* ===== 未连接：注册表单 ===== */
          <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5 space-y-2">
            <p className="text-[11px] font-medium text-slate-900/85">
              连接到同步服务器
            </p>
            <p className="text-[10px] text-slate-700/50 leading-relaxed">
              先用 Docker 部署 zbar-sync 服务，从启动日志复制 Master Token。
            </p>

            <label className="flex flex-col gap-0.5 text-[10px]">
              <span className="text-slate-700/55">服务器地址</span>
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
              <span className="text-slate-700/55">准入凭证 (Master Token)</span>
              <div className="flex items-center rounded-md bg-slate-900/5 border border-slate-900/10 focus-within:border-sky-400/60">
                <input
                  type={showMaster ? "text" : "password"}
                  value={regForm.master_token}
                  placeholder="docker logs 中的 MASTER_TOKEN"
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
              <span className="text-slate-700/55">设备名称</span>
              <input
                type="text"
                value={regForm.device_name}
                placeholder="如：work / home"
                onChange={(e) =>
                  setRegForm((f) => ({ ...f, device_name: e.target.value }))
                }
                className="px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none focus:border-sky-400/60"
              />
            </label>

            {regForm.server_url.startsWith("http://") && (
              <p className="text-[9px] text-amber-600/80 leading-relaxed">
                ⚠️ HTTP 明文传输，建议内网使用或配置 HTTPS 反向代理。
              </p>
            )}

            <button
              onClick={handleRegister}
              disabled={
                registering ||
                !regForm.server_url.trim() ||
                !regForm.master_token.trim() ||
                !regForm.device_name.trim()
              }
              className="w-full text-[11px] py-1.5 rounded-md bg-sky-500 text-white hover:bg-sky-600 disabled:opacity-40 transition-colors"
            >
              {registering ? "连接中…" : "连接并注册"}
            </button>
          </div>
        ) : (
          /* ===== 已连接：同步状态 + 数据管理 ===== */
          <>
            {/* 连接状态 */}
            <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
              <div className="flex items-center justify-between">
                <div className="flex items-center gap-1.5">
                  <span className="w-1.5 h-1.5 rounded-full bg-emerald-500" />
                  <span className="text-[11px] font-medium text-slate-900/85">
                    {config!.device_name}
                  </span>
                </div>
                <span className="text-[9px] text-slate-700/45 font-mono">
                  {config!.device_id.slice(0, 8)}
                </span>
              </div>
              <div className="mt-1.5 grid grid-cols-2 gap-1.5 text-[10px]">
                <div className="text-slate-700/55">
                  服务器
                  <div className="text-slate-900/80 truncate">
                    {config!.server_url}
                  </div>
                </div>
                <div className="text-slate-700/55">
                  上次同步
                  <div className="text-slate-900/80">
                    {timeAgo(config!.last_sync_at)}
                  </div>
                </div>
                <div className="text-slate-700/55">
                  待上传
                  <div className="text-slate-900/80">{pending} 条</div>
                </div>
                <div className="text-slate-700/55">
                  已传游标
                  <div className="text-slate-900/80">
                    #{config!.last_uploaded_rowid}
                  </div>
                </div>
              </div>

              {/* 同步模式 */}
              <div className="mt-2.5 pt-2 border-t border-slate-900/10">
                <div className="flex items-center justify-between">
                  <span className="text-[10px] text-slate-700/55">同步模式</span>
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
                        {m === "manual" ? "手动" : "自动"}
                      </button>
                    ))}
                  </div>
                </div>
                {modeDraft === "auto" && (
                  <label className="flex items-center justify-between mt-1.5 text-[10px]">
                    <span className="text-slate-700/55">间隔（秒）</span>
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
                      ? "同步中…"
                      : syncFlash
                        ? syncFlash
                        : "立即同步"}
                  </button>
                  <button
                    onClick={handleSaveMode}
                    className="text-[11px] px-2.5 py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
                  >
                    保存模式
                  </button>
                  <button
                    onClick={handleDisconnect}
                    className="text-[11px] px-2.5 py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                  >
                    断开
                  </button>
                </div>
              </div>
            </div>

            {/* 数据管理 */}
            <div className="rounded-lg bg-slate-900/5 border border-slate-900/10 p-2.5">
              <div className="flex items-center justify-between mb-1.5">
                <span className="text-[11px] font-medium text-slate-900/85">
                  数据管理
                </span>
                {cleanupStatus && (
                  <span className="text-[10px] text-slate-700/50">
                    共 {cleanupStatus.total_records} 条
                  </span>
                )}
              </div>

              <label className="flex flex-col gap-0.5 text-[10px] mb-2">
                <span className="text-slate-700/55">Master Token（操作清理用）</span>
                <input
                  type="password"
                  value={masterInput}
                  placeholder="粘贴 Master Token"
                  onChange={(e) => setMasterInput(e.target.value)}
                  className="px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] text-slate-900/90 placeholder:text-slate-700/35 focus:outline-none focus:border-sky-400/60"
                />
              </label>

              {/* 自动清理配置 */}
              <div className="flex items-center justify-between mb-1.5 py-1 border-t border-slate-900/10">
                <span className="text-[10px] text-slate-700/55">
                  自动清理
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
                  {cleanupStatus?.auto_config.auto_enabled ? "已开启" : "已关闭"}
                </button>
              </div>
              {cleanupStatus?.auto_config.auto_enabled && (
                <label className="flex items-center justify-between mb-2 text-[10px]">
                  <span className="text-slate-700/55">保留天数</span>
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
                          <span className="ml-1 text-sky-600/70">本机</span>
                        )}
                      </span>
                      <span className="flex items-center gap-1.5 shrink-0">
                        <span className="text-slate-700/45">
                          {d.record_count ?? 0} 条
                        </span>
                        <button
                          onClick={() => setRenameTarget(d)}
                          disabled={busy}
                          className="text-slate-700/40 hover:text-sky-600 transition-colors"
                          title="改名"
                        >
                          ✎
                        </button>
                        {d.device_id !== config!.device_id && (
                          <>
                            <button
                              onClick={() => setMergeSource(d)}
                              disabled={busy}
                              className="text-slate-700/50 hover:text-sky-600 transition-colors"
                              title="合并到其他设备"
                            >
                              合并
                            </button>
                            <button
                              onClick={() =>
                                setConfirmAction(`device:${d.device_id}`)
                              }
                              disabled={busy}
                              className="text-slate-700/40 hover:text-red-600 transition-colors"
                              title="删除此设备数据"
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
                  按时间清理
                </button>
                <button
                  onClick={() => setConfirmAction("all")}
                  disabled={busy}
                  className="flex-1 text-[10px] py-1 rounded-md bg-slate-900/5 text-slate-700/65 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                >
                  全部清空
                </button>
                <button
                  onClick={() => setConfirmAction("reset")}
                  disabled={busy}
                  className="flex-1 text-[10px] py-1 rounded-md bg-slate-900/5 text-slate-700/65 hover:bg-red-500/15 hover:text-red-700 transition-colors"
                >
                  重置
                </button>
              </div>
            </div>
          </>
        )}
      </div>

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
    </div>
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
          合并设备
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          将「{source.device_name}（{source.device_id.slice(0, 6)}）」的{" "}
          {source.record_count ?? 0} 条记录合并到：
        </p>
        <select
          value={target}
          onChange={(e) => setTarget(e.target.value)}
          className="w-full mb-2 px-1.5 py-1 rounded-md bg-slate-900/5 border border-slate-900/10 text-[11px] focus:outline-none focus:border-sky-400/60"
        >
          {candidates.map((d) => (
            <option key={d.device_id} value={d.device_id}>
              {d.device_name}（{d.device_id.slice(0, 6)}）
              {d.device_id === localDeviceId ? " · 本机" : ""}
            </option>
          ))}
        </select>
        {targetIsLocal ? (
          <p className="text-[10px] text-amber-700/80 leading-relaxed mb-2">
            合并到本机后，被合并的历史数据在本机"全部汇总"视图中可能不可见。建议改
            合并到该设备当前正在用的实例。
          </p>
        ) : (
          <p className="text-[10px] text-amber-700/80 leading-relaxed mb-2">
            来源设备的记录会转移到目标设备，来源设备将被删除，不可恢复。若来源设备仍
            在某台机器上同步，请到那台机器"断开"并重新注册。
          </p>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            取消
          </button>
          <button
            disabled={!target}
            onClick={() => onConfirm(target)}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40"
          >
            确认合并
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
  const [name, setName] = useState(device.device_name);
  const trimmed = name.trim();
  const valid = trimmed.length > 0 && trimmed.length <= 32;

  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          设备改名
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2">
          修改「{device.device_name}（{device.device_id.slice(0, 6)}）」的名称。
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
            取消
          </button>
          <button
            disabled={!valid}
            onClick={() => onConfirm(trimmed)}
            className="flex-1 text-[11px] py-1 rounded-md bg-sky-500 text-white hover:bg-sky-600 transition-colors disabled:opacity-40"
          >
            确认
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
  const [type, arg] = [action.split(":")[0], action.split(":")[1]];
  const [days, setDays] = useState(30);

  const title =
    type === "device"
      ? "删除设备数据"
      : type === "before"
        ? "按时间清理"
        : type === "all"
          ? "全部清空"
          : "重置服务器";

  const desc =
    type === "device"
      ? `将删除该设备的全部明细，不可恢复。`
      : type === "before"
        ? `将删除 ${days} 天前的所有数据，趋势图历史范围会缩短。不可恢复。`
        : type === "all"
          ? "将清空所有用量数据（保留设备注册），不可恢复。"
          : "将清空所有数据并删除所有设备，回到初始状态。不可恢复。";

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
            <span className="text-slate-700/55">保留天数</span>
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
            取消
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
            确认删除
          </button>
        </div>
      </div>
    </div>
  );
}

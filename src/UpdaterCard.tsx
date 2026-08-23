// 「关于与更新」卡片：当前版本 + 检查更新 + 后台下载进度 + 重启安装。
// 更新源在 tauri.conf.json 配了 GitHub / Gitee 双 endpoint，依次自动降级。
// 安装失败（如 macOS 未签名被 Gatekeeper 拦）降级为打开 Gitee 下载页。
import { useEffect, useState } from "react";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SettingsCard, BtnPrimary } from "./layout";
import { useI18n } from "./i18n";
import {
  checkAndDownloadInBackground,
  getUpdateState,
  installPendingUpdate,
  UPDATE_STATE_EVENT,
  type UpdatePhase,
} from "./updater";

const DOWNLOAD_PAGE = "https://gitee.com/codezwx/zai-floating-monitor/releases";

export function UpdaterCard() {
  const { t } = useI18n();
  const [version, setVersion] = useState("");
  const [phase, setPhase] = useState<UpdatePhase>(() => getUpdateState().phase);
  const [progress, setProgress] = useState(() => getUpdateState().progress);
  const [pendingVersion, setPendingVersion] = useState(
    () => getUpdateState().pendingVersion
  );
  const [readyVersion, setReadyVersion] = useState(
    () => getUpdateState().readyVersion
  );
  const [releaseNote, setReleaseNote] = useState(
    () => getUpdateState().releaseNote
  );
  const [error, setError] = useState<string | null>(() => getUpdateState().error);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
  }, []);

  useEffect(() => {
    const sync = () => {
      const s = getUpdateState();
      setPhase(s.phase);
      setProgress(s.progress);
      setPendingVersion(s.pendingVersion);
      setReadyVersion(s.readyVersion);
      setReleaseNote(s.releaseNote);
      setError(s.error);
    };
    window.addEventListener(UPDATE_STATE_EVENT, sync);
    return () => window.removeEventListener(UPDATE_STATE_EVENT, sync);
  }, []);

  const displayVersion = readyVersion ?? pendingVersion;
  const releaseNoteText = releaseNote?.trim() || null;

  const handleCheck = () => {
    checkAndDownloadInBackground({ silent: false });
  };

  const handleRestartUpdate = async () => {
    try {
      await installPendingUpdate();
    } catch {
      /* installPendingUpdate 已将 phase 设为 error */
    }
  };

  const handleOpenPage = () => {
    openUrl(DOWNLOAD_PAGE).catch(() => window.open(DOWNLOAD_PAGE, "_blank"));
  };

  const busy =
    phase === "checking" ||
    phase === "downloading" ||
    phase === "installing";

  return (
    <SettingsCard
      title={t("settings.aboutCard")}
      action={
        phase === "checking" ? (
          <span className="text-[9px] text-slate-500">{t("settings.checking")}</span>
        ) : (
          <BtnPrimary onClick={handleCheck} disabled={busy}>
            {t("settings.checkUpdate")}
          </BtnPrimary>
        )
      }
    >
      <div className="flex items-center gap-2 text-[10px]">
        <span className="text-slate-700/60">{t("settings.currentVersion")}</span>
        <span className="num font-medium text-slate-900/85">{version || "—"}</span>
        {phase === "uptodate" && (
          <span className="text-emerald-600">✓ {t("settings.upToDate")}</span>
        )}
        {displayVersion && phase !== "uptodate" && (
          <span className="text-sky-700 font-medium">
            {t("settings.newVersion", { v: displayVersion })}
          </span>
        )}
      </div>

      {phase === "downloading" && (
        <div className="mt-1.5">
          <p className="text-[9px] text-slate-600/80 mb-1">
            {t("settings.backgroundDownloading")}
          </p>
          <div className="flex items-center gap-2">
            <div className="flex-1 h-1.5 rounded-full bg-slate-900/10 overflow-hidden">
              <div
                className="h-full bg-sky-500 rounded-full transition-all"
                style={{ width: `${progress}%` }}
              />
            </div>
            <span className="num text-[9px] text-slate-600 shrink-0">
              {t("settings.downloading", { pct: progress })}
            </span>
          </div>
        </div>
      )}

      {phase === "ready" && readyVersion && (
        <div className="mt-1.5">
          <p className="text-[9px] text-sky-700/90 mb-1.5">
            {t("settings.updateReady", { v: readyVersion })}
          </p>
          <div className="flex items-center gap-2">
            <BtnPrimary onClick={handleRestartUpdate}>
              {t("settings.restartUpdate")}
            </BtnPrimary>
            <button
              onClick={handleOpenPage}
              className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors"
            >
              {t("settings.openDownloadPage")}
            </button>
          </div>
          {releaseNoteText && (
            <pre className="mt-1.5 max-h-24 overflow-y-auto text-[9px] text-slate-600/80 leading-relaxed whitespace-pre-wrap font-sans">
              {releaseNoteText}
            </pre>
          )}
        </div>
      )}

      {phase === "installing" && (
        <div className="mt-1.5">
          <span className="text-[9px] text-slate-600">{t("settings.installing")}</span>
        </div>
      )}

      {phase === "error" && error && (
        <div className="mt-1.5 flex items-center gap-2">
          <p className="text-[9px] text-rose-600 leading-relaxed break-all flex-1">
            {t("settings.updateFailed", { msg: error })}
          </p>
          <button
            onClick={handleOpenPage}
            className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors shrink-0"
          >
            {t("settings.openDownloadPage")}
          </button>
        </div>
      )}

      <p className="text-[8px] text-slate-700/40 mt-1.5">
        {t("settings.updateHint")}
      </p>
    </SettingsCard>
  );
}

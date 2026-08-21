// 「关于与更新」卡片：当前版本 + 检查更新 + 下载进度 + 安装。
// 更新源在 tauri.conf.json 配了 GitHub / Gitee 双 endpoint，依次自动降级。
// 安装失败（如 macOS 未签名被 Gatekeeper 拦）降级为打开 Gitee 下载页。
import { useEffect, useState } from "react";
import { check, type Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { getVersion } from "@tauri-apps/api/app";
import { openUrl } from "@tauri-apps/plugin-opener";
import { SettingsCard, BtnPrimary } from "./layout";
import { useI18n } from "./i18n";
import { UPDATE_AVAILABLE_KEY } from "./updater";

const DOWNLOAD_PAGE = "https://gitee.com/codezwx/zai-floating-monitor/releases";

type Phase =
  | "idle" // 初始（未检查或静默检查发现有新版）
  | "checking"
  | "uptodate"
  | "downloading"
  | "installing"
  | "error";

export function UpdaterCard() {
  const { t } = useI18n();
  const [version, setVersion] = useState("");
  const [update, setUpdate] = useState<Update | null>(null);
  const [phase, setPhase] = useState<Phase>("idle");
  const [progress, setProgress] = useState(0);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    getVersion()
      .then(setVersion)
      .catch(() => setVersion(""));
    // 启动静默检查已发现新版本（红点来源）：进入设置页时补一次检查，
    // 直接填出「发现新版本 + 下载安装」，省去用户再点一次
    try {
      if (localStorage.getItem(UPDATE_AVAILABLE_KEY)) {
        handleCheck();
      }
    } catch {
      /* 存储异常忽略 */
    }
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 有新版本提示时 title 悬浮显示说明
  const releaseNote = update?.body?.trim() || null;

  const handleCheck = async () => {
    setPhase("checking");
    setError(null);
    try {
      const u = await check();
      if (u?.available) {
        setUpdate(u);
        localStorage.setItem(UPDATE_AVAILABLE_KEY, u.version);
        setPhase("idle");
      } else {
        setUpdate(null);
        localStorage.removeItem(UPDATE_AVAILABLE_KEY);
        setPhase("uptodate");
      }
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  const handleDownloadInstall = async () => {
    if (!update) return;
    setPhase("downloading");
    setProgress(0);
    setError(null);
    try {
      let contentLength = 0;
      let downloaded = 0;
      await update.downloadAndInstall((event) => {
        switch (event.event) {
          case "Started":
            contentLength = event.data.contentLength ?? 0;
            break;
          case "Progress":
            downloaded += event.data.chunkLength;
            if (contentLength > 0) {
              setProgress(Math.min(100, Math.round((downloaded / contentLength) * 100)));
            }
            break;
          case "Finished":
            setProgress(100);
            break;
        }
      });
      setPhase("installing");
      // Windows NSIS 安装器启动前应用会自动退出；macOS/Linux 需主动重启
      await relaunch();
    } catch (e) {
      setError(String(e));
      setPhase("error");
    }
  };

  const handleOpenPage = () => {
    openUrl(DOWNLOAD_PAGE).catch(() => window.open(DOWNLOAD_PAGE, "_blank"));
  };

  return (
    <SettingsCard
      title={t("settings.aboutCard")}
      action={
        phase === "checking" ? (
          <span className="text-[9px] text-slate-500">{t("settings.checking")}</span>
        ) : (
          <BtnPrimary onClick={handleCheck} disabled={phase === "downloading" || phase === "installing"}>
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
        {update && (
          <span className="text-sky-700 font-medium">
            {t("settings.newVersion", { v: update.version })}
          </span>
        )}
      </div>

      {update && (
        <div className="mt-1.5">
          {phase === "downloading" || phase === "installing" ? (
            <div className="flex items-center gap-2">
              <div className="flex-1 h-1.5 rounded-full bg-slate-900/10 overflow-hidden">
                <div
                  className="h-full bg-sky-500 rounded-full transition-all"
                  style={{ width: `${progress}%` }}
                />
              </div>
              <span className="num text-[9px] text-slate-600 shrink-0">
                {phase === "installing"
                  ? t("settings.installing")
                  : t("settings.downloading", { pct: progress })}
              </span>
            </div>
          ) : (
            <div className="flex items-center gap-2">
              <BtnPrimary onClick={handleDownloadInstall}>
                {t("settings.downloadInstall")}
              </BtnPrimary>
              <button
                onClick={handleOpenPage}
                className="text-[9px] px-1.5 py-0.5 rounded bg-sky-500/10 text-sky-700/80 hover:bg-sky-500/20 transition-colors"
              >
                {t("settings.openDownloadPage")}
              </button>
            </div>
          )}
          {releaseNote && (
            <pre
              className="mt-1.5 max-h-24 overflow-y-auto text-[9px] text-slate-600/80 leading-relaxed whitespace-pre-wrap font-sans"
            >
              {releaseNote}
            </pre>
          )}
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

// 应用内更新：定时检查 + 后台下载 + 设置入口红点（下载完成后）+ 设置页重启安装。
import { Update } from "@tauri-apps/plugin-updater";
import { relaunch } from "@tauri-apps/plugin-process";
import { invoke } from "@tauri-apps/api/core";
import { detectLocale, loadLocale } from "./i18n/locale";

/** Rust check_update 命令返回的更新元数据（字段与官方 Update 构造器对齐） */
interface UpdateCheckMeta {
  rid: number;
  currentVersion: string;
  version: string;
  /** Rust Option::None 序列化为 null */
  date: string | null;
  body: string | null;
  rawJson: Record<string, unknown>;
}

/** localStorage：后台下载完成、待安装的版本号 */
export const UPDATE_READY_KEY = "zbar.updateReady";

/** 下载完成待安装时广播（StatsPanel 红点即时刷新） */
export const UPDATE_READY_EVENT = "zbar:update-ready";

/** 更新状态变化时广播（UpdaterCard 同步进度/阶段） */
export const UPDATE_STATE_EVENT = "zbar:update-state";

export type UpdatePhase =
  | "idle"
  | "checking"
  | "downloading"
  | "ready"
  | "uptodate"
  | "installing"
  | "error";

export interface UpdateState {
  phase: UpdatePhase;
  progress: number;
  readyVersion: string | null;
  pendingVersion: string | null;
  releaseNote: string | null;
  error: string | null;
}

let pendingUpdate: Update | null = null;

let state: UpdateState = {
  phase: "idle",
  progress: 0,
  readyVersion: null,
  pendingVersion: null,
  releaseNote: null,
  error: null,
};

// 进程重启后内存中的 Update 已失效，清除残留的 ready 标记，由调度器重新下载
try {
  localStorage.removeItem(UPDATE_READY_KEY);
  localStorage.removeItem("zbar.updateAvailable");
} catch {
  /* 忽略存储异常 */
}

function notifyState() {
  window.dispatchEvent(new Event(UPDATE_STATE_EVENT));
}

function setState(partial: Partial<UpdateState>) {
  state = { ...state, ...partial };
  notifyState();
}

export function getUpdateState(): UpdateState {
  return { ...state };
}

function clearUpdateReady() {
  try {
    const had = localStorage.getItem(UPDATE_READY_KEY);
    localStorage.removeItem(UPDATE_READY_KEY);
    if (had) {
      window.dispatchEvent(new Event(UPDATE_READY_EVENT));
    }
  } catch {
    /* 忽略存储异常 */
  }
}

async function closePending() {
  if (!pendingUpdate) return;
  try {
    await pendingUpdate.close();
  } catch {
    /* 忽略关闭异常 */
  }
  pendingUpdate = null;
}

function markReady(version: string) {
  try {
    localStorage.setItem(UPDATE_READY_KEY, version);
  } catch {
    /* 忽略存储异常 */
  }
  setState({ phase: "ready", readyVersion: version, progress: 100 });
  window.dispatchEvent(new Event(UPDATE_READY_EVENT));
}

/** 后台（或手动）检查更新并在发现新版本时自动下载；下载完成后写 localStorage 并广播红点 */
export async function checkAndDownloadInBackground(opts?: {
  silent?: boolean;
}): Promise<void> {
  const silent = opts?.silent ?? true;

  if (
    state.phase === "checking" ||
    state.phase === "downloading" ||
    state.phase === "installing"
  ) {
    return;
  }
  if (state.phase === "ready" && state.readyVersion) {
    return;
  }

  setState({ phase: "checking", error: null });

  try {
    // 按界面语言选更新源（中文优先 Gitee，英文优先 GitHub，另一源兜底）：
    // 官方 check() 的 endpoints 只能来自静态配置，改用自定义 Rust 命令动态注入顺序
    const locale = loadLocale() ?? detectLocale();
    const meta = await invoke<UpdateCheckMeta | null>("check_update", { locale });
    // null（无日期/说明）归一为 undefined，对齐官方 UpdateMetadata 的可选字段类型
    const update = meta
      ? new Update({
          ...meta,
          date: meta.date ?? undefined,
          body: meta.body ?? undefined,
        })
      : null;
    if (!update?.available) {
      await closePending();
      clearUpdateReady();
      setState({
        phase: silent ? "idle" : "uptodate",
        progress: 0,
        readyVersion: null,
        pendingVersion: null,
        releaseNote: null,
      });
      return;
    }

    if (pendingUpdate && state.readyVersion !== update.version) {
      await closePending();
      clearUpdateReady();
    }

    pendingUpdate = update;
    setState({
      phase: "downloading",
      progress: 0,
      pendingVersion: update.version,
      releaseNote: update.body?.trim() || null,
      readyVersion: null,
    });

    let contentLength = 0;
    let downloaded = 0;
    await update.download((event) => {
      switch (event.event) {
        case "Started":
          contentLength = event.data.contentLength ?? 0;
          break;
        case "Progress":
          downloaded += event.data.chunkLength;
          if (contentLength > 0) {
            setState({
              progress: Math.min(
                100,
                Math.round((downloaded / contentLength) * 100)
              ),
            });
          }
          break;
        case "Finished":
          setState({ progress: 100 });
          break;
      }
    });

    markReady(update.version);
  } catch (e) {
    await closePending();
    clearUpdateReady();
    if (silent) {
      setState({
        phase: "idle",
        progress: 0,
        readyVersion: null,
        pendingVersion: null,
        releaseNote: null,
        error: null,
      });
    } else {
      setState({ phase: "error", error: String(e) });
    }
  }
}

/** 安装已下载的更新包并重启应用 */
export async function installPendingUpdate(): Promise<void> {
  if (!pendingUpdate || state.phase !== "ready") {
    throw new Error("No pending update ready to install");
  }

  setState({ phase: "installing", error: null });

  try {
    await pendingUpdate.install();
    clearUpdateReady();
    pendingUpdate = null;
    setState({
      phase: "idle",
      progress: 0,
      readyVersion: null,
      pendingVersion: null,
      releaseNote: null,
    });
    // Windows NSIS 安装器启动前应用会自动退出；macOS/Linux 需主动重启
    await relaunch();
  } catch (e) {
    setState({ phase: "error", error: String(e) });
    throw e;
  }
}

/** 启动更新调度：首次延迟 10s，之后每 1 小时检查并后台下载 */
export function startUpdateScheduler(): () => void {
  const initialTimer = window.setTimeout(() => {
    checkAndDownloadInBackground({ silent: true });
  }, 10_000);
  const hourlyInterval = window.setInterval(() => {
    checkAndDownloadInBackground({ silent: true });
  }, 3_600_000);
  return () => {
    window.clearTimeout(initialTimer);
    window.clearInterval(hourlyInterval);
  };
}

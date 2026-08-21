// 应用内更新：启动静默检查（App.tsx 调用）与设置入口红点（StatsPanel 消费）。
// 检查/下载/安装的完整交互在 UpdaterCard.tsx。
import { check } from "@tauri-apps/plugin-updater";

/** localStorage：启动静默检查发现的新版本号（无值 = 无更新或未检查） */
export const UPDATE_AVAILABLE_KEY = "zbar.updateAvailable";

/** 静默检查写入后广播的窗口事件（StatsPanel 红点即时刷新） */
export const UPDATE_EVENT = "zbar:update-available";

/** 启动静默检查：有新版本记 localStorage 并广播；无版本/失败一律静默，
 *  不打扰用户（失败时用户仍可在设置页手动检查）。 */
export async function silentCheckForUpdate(): Promise<void> {
  try {
    const update = await check();
    if (update?.available) {
      localStorage.setItem(UPDATE_AVAILABLE_KEY, update.version);
      window.dispatchEvent(new Event(UPDATE_EVENT));
    } else {
      localStorage.removeItem(UPDATE_AVAILABLE_KEY);
    }
  } catch {
    // 网络不通 / 端点全挂：静默跳过
  }
}

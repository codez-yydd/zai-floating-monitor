/** 订阅重置时间的展示偏好：倒计时 / 具体时间点可同时开启，
 *  仅影响界面展示，不影响采集与同步。 */
import { useState } from "react";

export type ResetDisplay = { countdown: boolean; datetime: boolean };

const STORAGE_KEY = "zbar-reset-display";

/** 默认仅倒计时（= 历史行为），升级后渲染零变化。 */
const DEFAULT_DISPLAY: ResetDisplay = {
  countdown: true,
  datetime: false,
};

/** 读取展示偏好；缺失或损坏字段按字段回退默认值，保证升级后行为不变。 */
export function loadResetDisplay(): ResetDisplay {
  try {
    const raw = localStorage.getItem(STORAGE_KEY);
    if (!raw) return { ...DEFAULT_DISPLAY };
    const parsed = JSON.parse(raw) as Partial<ResetDisplay>;
    return {
      countdown: parsed.countdown !== false,
      datetime: parsed.datetime === true,
    };
  } catch {
    return { ...DEFAULT_DISPLAY };
  }
}

/** 设置页切换后立即保存，异常时静默降级为当前会话内状态。 */
export function saveResetDisplay(display: ResetDisplay): void {
  try {
    localStorage.setItem(STORAGE_KEY, JSON.stringify(display));
  } catch {
    // 隐私模式或存储配额不足不应阻断设置页操作。
  }
}

/** 各展示组件挂载时读取一次（惰性初始化）。视图互斥渲染、
 *  返回统计页必然重挂载，无需跨视图实时同步。 */
export function useResetDisplay(): ResetDisplay {
  return useState(loadResetDisplay)[0];
}

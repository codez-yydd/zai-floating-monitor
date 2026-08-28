/**
 * 窗口尺寸偏好（百分比记忆）：
 * 窗口宽高以「当前显示器工作区百分比」（0~1 浮点）存储，跨分辨率 / 显示器
 * 缩放迁移更稳定；localStorage 持久化，启动时按当前工作区换算回逻辑像素恢复。
 */

import {
  LogicalPosition,
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
  type Monitor,
} from "@tauri-apps/api/window";

/** 窗口尺寸百分比类型（w/h 均为 0~1 浮点） */
export type WinSizePct = { w: number; h: number };

/** 窗口尺寸持久化键（存 JSON：{"w":0~1,"h":0~1}） */
export const WIN_SIZE_KEY = "zbar-win-size";

/** 基准百分比：默认 300×500 窗口在 14" MBP 逻辑工作区的占比圆整 */
export const BASE_PCT: WinSizePct = { w: 0.2, h: 0.51 };

/** 尺寸档位表：倍率作用于 BASE_PCT（w/h 同倍率），labelKey 供设置页渲染（i18n 键由后续任务添加） */
export const WIN_PRESETS: {
  scale: number;
  labelKey:
    | "settings.winSizeSmall"
    | "settings.winSizeStandard"
    | "settings.winSizeLarge"
    | "settings.winSizeXl";
}[] = [
  { scale: 0.9, labelKey: "settings.winSizeSmall" },
  { scale: 1, labelKey: "settings.winSizeStandard" },
  { scale: 1.1, labelKey: "settings.winSizeLarge" },
  { scale: 1.25, labelKey: "settings.winSizeXl" },
];

/** 窗口最小逻辑尺寸（与 tauri.conf.json 的 minWidth/minHeight 一致） */
export const MIN_PX = { w: 260, h: 430 };
/** 窗口最大逻辑尺寸（与 tauri.conf.json 的 maxWidth/maxHeight 一致） */
export const MAX_PX = { w: 420, h: 760 };

/** 百分比域上限：超过 0.65 视为脏数据（防串值 / 异常存储）；
 *  上限需覆盖最大档位落盘值（scale 1.25 × BASE_PCT.h 0.51 = 0.6375） */
export const PCT_MAX = 0.65;

/** 是否 Windows（与 StatsPanel 的 isWindows 同款判断）：兜底估算工作区时的系统栏高度扣除 */
const isWindows =
  typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent);

/** 百分比域校验：w/h 都是 (0, 0.65] 区间内的有限数 */
function isValidPct(pct: WinSizePct): boolean {
  return (
    Number.isFinite(pct.w) &&
    Number.isFinite(pct.h) &&
    pct.w > 0 &&
    pct.w <= PCT_MAX &&
    pct.h > 0 &&
    pct.h <= PCT_MAX
  );
}

/** 读取窗口尺寸百分比：无值、解析失败、非有限数或超出合理域一律视为未存储（返回 null） */
export function loadWinSizePct(): WinSizePct | null {
  try {
    const raw = localStorage.getItem(WIN_SIZE_KEY);
    if (!raw) return null;
    const v = JSON.parse(raw) as { w?: unknown; h?: unknown };
    if (typeof v !== "object" || v === null) return null;
    const pct: WinSizePct = { w: Number(v.w), h: Number(v.h) };
    return isValidPct(pct) ? pct : null;
  } catch {
    return null;
  }
}

/** 持久化窗口尺寸百分比（保留至多 4 位小数，避免浮点尾数膨胀） */
export function persistWinSizePct(pct: WinSizePct): void {
  try {
    localStorage.setItem(
      WIN_SIZE_KEY,
      JSON.stringify({ w: +pct.w.toFixed(4), h: +pct.h.toFixed(4) }),
    );
  } catch {
    /* 忽略：QuotaExceededError、隐私模式等（对齐 cache.ts） */
  }
}

/** 取显示器逻辑工作区（x/y/w/h）：workArea 缺失时用整屏尺寸估算并扣系统栏高度 */
function logicalWorkArea(monitor: Monitor): {
  x: number;
  y: number;
  w: number;
  h: number;
} {
  const sf = monitor.scaleFactor || 1;
  const was = monitor.workArea?.size;
  const wap = monitor.workArea?.position;
  if (was && wap && was.width > 0 && was.height > 0) {
    return {
      x: wap.x / sf,
      y: wap.y / sf,
      w: was.width / sf,
      h: was.height / sf,
    };
  }
  // 兜底：整屏物理尺寸 ÷ 缩放得逻辑值，再扣系统栏高度
  // （macOS 菜单栏约 25、Windows 任务栏约 48）
  return {
    x: monitor.position.x / sf,
    y: monitor.position.y / sf,
    w: monitor.size.width / sf,
    h: monitor.size.height / sf - (isWindows ? 48 : 25),
  };
}

/** 百分比 → 逻辑像素：round(pct × 工作区) 后 clamp 到最小 / 最大尺寸；monitor 为 null 时返回 null */
function pctToPx(
  pct: WinSizePct,
  monitor: Monitor | null,
): { w: number; h: number } | null {
  if (!monitor) return null;
  const wa = logicalWorkArea(monitor);
  return {
    w: Math.min(Math.max(Math.round(pct.w * wa.w), MIN_PX.w), MAX_PX.w),
    h: Math.min(Math.max(Math.round(pct.h * wa.h), MIN_PX.h), MAX_PX.h),
  };
}

/** 逻辑像素 → 百分比（反向换算，拖拽落盘用）：monitor 为 null 或工作区非法时返回 null */
export function pxToPct(
  w: number,
  h: number,
  monitor: Monitor | null,
): WinSizePct | null {
  if (!monitor) return null;
  const wa = logicalWorkArea(monitor);
  if (wa.w <= 0 || wa.h <= 0) return null;
  return { w: w / wa.w, h: h / wa.h };
}

/** 落盘专用 clamp：w/h 上限动态取「PCT_MAX 与 MAX_PX 占逻辑工作区比例」的较大值。
 *  纯 PCT_MAX 会误伤大屏上合法可达的 px 尺寸（如 14" MBP 顶格高 760 ≈ 0.804 > 0.65，
 *  会被压回 0.65 导致重启恢复矮约 146px、记忆不准）；动态上限保证「用户真实拖到的
 *  合法 px 尺寸」忠实落盘，PCT_MAX 只拦真正的脏数据。
 *  monitor 为 null（拿不到工作区）时退回 PCT_MAX。 */
export function clampPctForPersist(
  pct: WinSizePct,
  monitor: Monitor | null,
): WinSizePct {
  const wa = monitor ? logicalWorkArea(monitor) : null;
  const maxW = wa && wa.w > 0 ? Math.max(PCT_MAX, MAX_PX.w / wa.w) : PCT_MAX;
  const maxH = wa && wa.h > 0 ? Math.max(PCT_MAX, MAX_PX.h / wa.h) : PCT_MAX;
  return {
    w: Math.min(Math.max(pct.w, 0), maxW),
    h: Math.min(Math.max(pct.h, 0), maxH),
  };
}

/** 启动恢复：读百分比换算像素后 setSize；未存储时直接 return（保持 conf 默认 300×500，零回归） */
export async function restoreWindowSize(): Promise<void> {
  try {
    const pct = loadWinSizePct();
    if (!pct) return;
    const px = pctToPx(pct, await currentMonitor());
    if (!px) return;
    await getCurrentWindow().setSize(new LogicalSize(px.w, px.h));
  } catch {
    /* 纯浏览器（npm run dev）无 Tauri IPC 或调用失败：静默保持默认尺寸 */
  }
}

/** 应用尺寸百分比（设置页档位切换用）：换算像素 setSize，并做右边界夹取——
 *  尺寸放大后右缘溢出工作区时左移 x（保持顶边 y 不变），避免面板探出屏幕外 */
export async function applyWindowPct(pct: WinSizePct): Promise<void> {
  try {
    const monitor = await currentMonitor();
    const px = pctToPx(pct, monitor);
    if (!px) return;
    const win = getCurrentWindow();
    await win.setSize(new LogicalSize(px.w, px.h));
    if (!monitor) return;
    const wa = logicalWorkArea(monitor);
    const sf = monitor.scaleFactor || 1;
    const pos = await win.outerPosition(); // 物理像素
    if (pos.x / sf + px.w > wa.x + wa.w) {
      await win.setPosition(
        new LogicalPosition(
          Math.round(wa.x + wa.w - px.w),
          Math.round(pos.y / sf),
        ),
      );
    }
  } catch {
    /* 忽略：设置失败静默，不打扰用户 */
  }
}

/** 读当前窗口尺寸换算百分比（供设置页判定档位 / 自定义态，i18n 由后续任务接线） */
export async function currentWinSizePct(): Promise<WinSizePct | null> {
  try {
    const monitor = await currentMonitor();
    if (!monitor) return null;
    const size = await getCurrentWindow().outerSize(); // 物理像素
    const sf = monitor.scaleFactor || 1;
    return pxToPct(size.width / sf, size.height / sf, monitor);
  } catch {
    return null;
  }
}

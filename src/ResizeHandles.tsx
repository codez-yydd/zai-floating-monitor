/**
 * 边缘拖拽热区：面板四边 / 四角的无形热区，pointerdown 后交给原生
 * startResizeDragging（8 向）调整窗口尺寸；结果换算为工作区百分比落盘
 * （windowSize.ts），成功后广播 zbar-win-size-changed 供设置页刷新档位。
 *
 * 落盘通道有两条：
 * 1. 会话终点：startResizeDragging 返回的 promise resolve（拖拽会话结束）后
 *    读最终窗口尺寸忠实落盘并复位会话标志——标志复位只在此处 / catch，
 *    中途停顿 >400ms 后继续拖依然有效；
 * 2. onResized 400ms 防抖：d.ts 对 promise 语义仅写「操作的成功或失败」，
 *    未明确承诺会话结束才 resolve，故保守保留此通道存中间值（无害，
 *    最终值由通道 1 覆盖）；此处不复位会话标志。
 *
 * 防误存：lib.rs 唤起面板时的 set_size(±1) 重绘 hack 未经热区，
 * userResizeSession 保持 false，其触发的 onResized 不进入存储通道。
 */

import { useEffect, type CSSProperties } from "react";
import {
  currentMonitor,
  getCurrentWindow,
  type Monitor,
} from "@tauri-apps/api/window";
import {
  clampPctForPersist,
  currentWinSizePct,
  persistWinSizePct,
  pxToPct,
  type WinSizePct,
} from "./windowSize";

/** 拖拽落盘成功后的广播事件：设置页窗口大小卡片监听以刷新档位胶囊 */
export const WIN_SIZE_CHANGED_EVENT = "zbar-win-size-changed";

/** Tauri ResizeDirection 联合（@tauri-apps/api 2.11.1 未导出该类型，本地等价声明） */
type TauriResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

/** 热区描述：四边 6px 贯穿全宽 / 全高，四角 12×12；角排在边之后渲染，
 *  自然盖在边交叠处优先命中 */
const HANDLES: { dir: TauriResizeDirection; style: CSSProperties }[] = [
  { dir: "North", style: { top: 0, left: 0, right: 0, height: 6, cursor: "ns-resize" } },
  { dir: "South", style: { bottom: 0, left: 0, right: 0, height: 6, cursor: "ns-resize" } },
  { dir: "West", style: { top: 0, bottom: 0, left: 0, width: 6, cursor: "ew-resize" } },
  { dir: "East", style: { top: 0, bottom: 0, right: 0, width: 6, cursor: "ew-resize" } },
  { dir: "NorthWest", style: { top: 0, left: 0, width: 12, height: 12, cursor: "nwse-resize" } },
  { dir: "NorthEast", style: { top: 0, right: 0, width: 12, height: 12, cursor: "nesw-resize" } },
  { dir: "SouthWest", style: { bottom: 0, left: 0, width: 12, height: 12, cursor: "nesw-resize" } },
  { dir: "SouthEast", style: { bottom: 0, right: 0, width: 12, height: 12, cursor: "nwse-resize" } },
];

/** 拖拽静止后的落盘防抖（ms）：拖拽期间连续 onResized 不断重置计时 */
const PERSIST_DEBOUNCE_MS = 400;

/** 会话标志：热区 pointerdown 置 true、拖拽会话 promise settle 后复位；
 *  非用户拖拽触发的 onResized（lib.rs 重绘 hack、启动恢复 setSize 等）
 *  一律不落盘 */
let userResizeSession = false;

/** clamp（动态上限，见 clampPctForPersist）→ 落盘 → 广播设置页刷新 */
function persistPct(pct: WinSizePct, monitor: Monitor): void {
  persistWinSizePct(clampPctForPersist(pct, monitor));
  window.dispatchEvent(new CustomEvent(WIN_SIZE_CHANGED_EVENT));
}

/** 热区按下：交原生 8 向 resize；落盘与标志复位绑定拖拽会话 promise settle */
const startResize = (dir: TauriResizeDirection) => {
  userResizeSession = true;
  getCurrentWindow()
    .startResizeDragging(dir)
    .then(async () => {
      // 拖拽会话终点：读最终窗口尺寸忠实落盘（覆盖防抖通道存的中间值），
      // 复位会话标志只在此路径，保证停顿 >400ms 后继续拖不丢最终尺寸
      try {
        const [pct, monitor] = await Promise.all([
          currentWinSizePct(),
          currentMonitor(),
        ]);
        if (pct && monitor) persistPct(pct, monitor);
      } catch {
        /* 静默：落盘失败不影响本次拖拽结果 */
      } finally {
        userResizeSession = false;
      }
    })
    .catch(() => {
      // 原生 resize 调用失败：复位会话标志，避免残留误存
      userResizeSession = false;
    });
};

/** 边缘拖拽热区组件：无可见 UI，仅提供 8 向隐形热区与尺寸落盘 */
export function ResizeHandles() {
  useEffect(() => {
    let timer: number | undefined;
    let disposed = false;
    let unlisten: (() => void) | null = null;

    getCurrentWindow()
      .onResized(({ payload }) => {
        if (!userResizeSession) return; // 非用户拖拽触发的尺寸变化不落盘
        // 400ms 防抖：拖拽期间连续触发不断重置计时，静止后存中间值
        // （无害——会话终点路径会用最终尺寸覆盖）；此处不复位会话标志，
        // 否则停顿 >400ms 后继续拖的 onResized 会被拦截、最终尺寸丢失
        window.clearTimeout(timer);
        timer = window.setTimeout(async () => {
          try {
            const monitor = await currentMonitor();
            if (!monitor) return;
            const sf = monitor.scaleFactor || 1;
            const pct = pxToPct(
              payload.width / sf,
              payload.height / sf,
              monitor,
            );
            if (!pct || !Number.isFinite(pct.w) || !Number.isFinite(pct.h)) return;
            persistPct(pct, monitor);
          } catch {
            /* 静默：落盘失败不影响本次拖拽结果 */
          }
        }, PERSIST_DEBOUNCE_MS);
      })
      .then((fn) => {
        // StrictMode 卸载先于注册完成时立即注销，避免泄漏第二个监听
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 纯浏览器（npm run dev）无 Tauri IPC：静默 */
      });

    return () => {
      disposed = true;
      window.clearTimeout(timer);
      unlisten?.();
    };
  }, []);

  return (
    // 全 view 共用的边缘热区层：fixed 相对 #root（transform 缩放容器）定位，
    // 随 --ui-scale 同步缩放；z-60 高于现有 z-50 浮层；容器不拦截事件
    <div className="pointer-events-none fixed inset-0 z-60">
      {HANDLES.map((h) => (
        <div
          key={h.dir}
          className="pointer-events-auto absolute"
          style={h.style}
          onPointerDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            startResize(h.dir);
          }}
        />
      ))}
    </div>
  );
}

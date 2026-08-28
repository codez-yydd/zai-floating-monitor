/**
 * 边缘拖拽热区：面板四边 / 四角的无形热区，8 向调整窗口尺寸；结果换算为工作区
 * 百分比落盘（windowSize.ts），成功后广播 zbar-win-size-changed 供设置页刷新档位。
 *
 * 平台分流：
 * - 明确识别为 Windows / macOS 时走前端 JS 自实现 resize 会话。原因：
 *   macOS 上 tao 0.35.3 的 drag_resize_window 返回 NotSupported，且
 *   tauri-runtime-wry 以 `let _ =` 吞错，startResizeDragging 静默无效，用户实际
 *   靠 AppKit 给 Borderless|Resizable 窗口的系统级边缘热区（约 3~5pt）拖动，
 *   前端热区内、系统热区外的窄条拖不动；Windows 上 tao 的 startResizeDragging
 *   是 GetCursorPos + ReleaseCapture + PostMessageW(WM_NCLBUTTONDOWN) 异步模拟
 *   系统消息，跨 WebView2 进程边界存在固有竞态（捕获在 WebView2 HWND 上释放
 *   不掉、IPC 延迟期间鼠标状态变化），偶发拖不动。JS 会话改为 pointerdown 采集
 *   几何快照（monitor / outerSize / outerPosition，物理 ÷ scaleFactor 换逻辑值）
 *   + pointermove 按 8 向增量 setSize / setPosition（Logical 单位），并用
 *   setPointerCapture 锁定事件流：无系统消息模拟、无跨进程竞态。
 * - 其余平台（Linux、识别失败）保留原生 startResizeDragging（tao 有实现，且
 *   Wayland 下程序化 setPosition 受限），行为与旧版一致。
 *
 * 落盘通道有两条：
 * 1. 会话终点：JS 路径 pointerup 读实际 outerSize 落盘终值（防 clamp 差异）；
 *    原生路径由 startResizeDragging 的 promise settle 后落盘；
 * 2. onResized 400ms 防抖：兜底通道。系统热区拖拽（macOS AppKit 边缘热区 /
 *    Windows WM_NCHITTEST）时 mouseDown 不进 WebView，JS 会话与原生 promise
 *    均不触发，只有此通道能落盘；拖拽期间存中间值无害，最终值由通道 1 覆盖。
 *
 * 防误存论证（onResized 防抖通道不设「用户会话」门槛的依据）：全项目程序化
 * set_size 仅三处——lib.rs show 后重绘 hack（±1 后立即还原原值）、启动
 * restoreWindowSize（恢复的就是记忆值）、设置页 applyWindowPct（落盘档位值即
 * 用户意图值），三者防抖后的稳定值都等于应存值，无误存。
 */

import {
  useEffect,
  type CSSProperties,
  type PointerEvent as ReactPointerEvent,
} from "react";
import {
  LogicalPosition,
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
  type Monitor,
} from "@tauri-apps/api/window";
import {
  MAX_PX,
  MIN_PX,
  clampPctForPersist,
  currentWinSizePct,
  logicalWorkArea,
  persistWinSizePct,
  pxToPct,
  type WinSizePct,
} from "./windowSize";

/** 拖拽落盘成功后的广播事件：设置页窗口大小卡片监听以刷新档位胶囊 */
export const WIN_SIZE_CHANGED_EVENT = "zbar-win-size-changed";

/** 是否 Windows（沿用 windowSize.ts 的判断风格：navigator.userAgent） */
const isWindows =
  typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent);
/** 是否 macOS：platform 优先（Safari/WebView 标准字段），缺失时退回 UA */
const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);

/** JS resize 会话路径开关：明确识别为 Windows / macOS 才启用；
 *  其余平台（Linux、识别失败）一律走原生 startResizeDragging，零回归 */
const USE_JS_RESIZE = isWindows || isMac;

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

/** 热区视觉厚度（逻辑 px）：边 8、角 14，用 calc 除以 --ui-scale 抵消 #root 的
 *  transform: scale(var(--ui-scale))，任意字体档位下视觉厚度恒定（同
 *  index.css 的 border-radius 补偿写法）；角排在边之后渲染，自然盖在边交叠处
 *  优先命中 */
const EDGE_PX = "calc(8px / var(--ui-scale))";
const CORNER_PX = "calc(14px / var(--ui-scale))";

const HANDLES: { dir: TauriResizeDirection; style: CSSProperties }[] = [
  { dir: "North", style: { top: 0, left: 0, right: 0, height: EDGE_PX, cursor: "ns-resize" } },
  { dir: "South", style: { bottom: 0, left: 0, right: 0, height: EDGE_PX, cursor: "ns-resize" } },
  { dir: "West", style: { top: 0, bottom: 0, left: 0, width: EDGE_PX, cursor: "ew-resize" } },
  { dir: "East", style: { top: 0, bottom: 0, right: 0, width: EDGE_PX, cursor: "ew-resize" } },
  { dir: "NorthWest", style: { top: 0, left: 0, width: CORNER_PX, height: CORNER_PX, cursor: "nwse-resize" } },
  { dir: "NorthEast", style: { top: 0, right: 0, width: CORNER_PX, height: CORNER_PX, cursor: "nesw-resize" } },
  { dir: "SouthWest", style: { bottom: 0, left: 0, width: CORNER_PX, height: CORNER_PX, cursor: "nesw-resize" } },
  { dir: "SouthEast", style: { bottom: 0, right: 0, width: CORNER_PX, height: CORNER_PX, cursor: "nwse-resize" } },
];

/** 拖拽静止后的落盘防抖（ms）：拖拽期间连续 onResized 不断重置计时 */
const PERSIST_DEBOUNCE_MS = 400;

/** clamp（动态上限，见 clampPctForPersist）→ 落盘 → 广播设置页刷新 */
function persistPct(pct: WinSizePct, monitor: Monitor): void {
  persistWinSizePct(clampPctForPersist(pct, monitor));
  window.dispatchEvent(new CustomEvent(WIN_SIZE_CHANGED_EVENT));
}

/** 原生 resize（Linux、识别失败平台及 JS 会话快照失败兜底）：交系统
 *  startResizeDragging；落盘绑定拖拽会话 promise settle */
const startResize = (dir: TauriResizeDirection) => {
  getCurrentWindow()
    .startResizeDragging(dir)
    .then(async () => {
      // 拖拽会话终点：读最终窗口尺寸忠实落盘（覆盖防抖通道存的中间值）
      try {
        const [pct, monitor] = await Promise.all([
          currentWinSizePct(),
          currentMonitor(),
        ]);
        if (pct && monitor) persistPct(pct, monitor);
      } catch {
        /* 静默：落盘失败不影响本次拖拽结果 */
      }
    })
    .catch(() => {
      /* 原生 resize 调用失败（纯浏览器无 IPC 等）：静默 */
    });
};

/** JS resize 会话快照：pointerdown 一次性采集；全程以此为基准做「快照 + 最新
 *  指针增量」的绝对赋值计算，每帧目标值互不依赖，乱序到达也不会累积误差 */
type JsResizeSession = {
  dir: TauriResizeDirection;
  pointerId: number;
  /** 指针起点（client 坐标，CSS 逻辑像素） */
  startX: number;
  startY: number;
  /** 窗口起点位置：outerPosition 物理 ÷ scaleFactor 得逻辑值 */
  winX: number;
  winY: number;
  /** 窗口起点尺寸：outerSize 物理 ÷ scaleFactor 得逻辑值 */
  winW: number;
  winH: number;
  /** 逻辑工作区左 / 上边界（N / W 方向拖动的位置下限） */
  waX: number;
  waY: number;
  /** rAF 节流：最新一次 pointermove 坐标，null 表示尚无待应用事件 */
  pendingX: number | null;
  pendingY: number | null;
  /** 已排队未执行的 rAF id，null 表示当前无排队 */
  rafId: number | null;
};

/** 当前 JS resize 会话（同一时刻至多一个），null 表示空闲 */
let jsSession: JsResizeSession | null = null;

/** 按方向增量计算并应用窗口几何（每动画帧至多一次，fire-and-forget 不阻塞输入） */
function applyJsResizeGeometry(s: JsResizeSession): void {
  if (s.pendingX === null || s.pendingY === null) return;
  const dx = s.pendingX - s.startX;
  const dy = s.pendingY - s.startY;
  const dir = s.dir;
  let w = s.winW;
  let h = s.winH;
  let x = s.winX;
  let y = s.winY;

  // 8 向增量：E/S 加增量、W/N 减增量（W/N 拖大时窗口反向扩张、边缘外移）
  if (dir.includes("East")) w = s.winW + dx;
  if (dir.includes("West")) w = s.winW - dx;
  if (dir.includes("South")) h = s.winH + dy;
  if (dir.includes("North")) h = s.winH - dy;

  // clamp 到配置的最小 / 最大尺寸（与 tauri.conf.json 的 min/max 一致）
  w = Math.min(Math.max(w, MIN_PX.w), MAX_PX.w);
  h = Math.min(Math.max(h, MIN_PX.h), MAX_PX.h);

  // N / W 联动 position，保持对边（S / E 边）不动
  if (dir.includes("West")) x = s.winX + (s.winW - w);
  if (dir.includes("North")) y = s.winY + (s.winH - h);

  // 工作区边界 clamp（仅 N / W 相关）：顶 / 左缘不许越过工作区上 / 左沿。
  // 顶 / 左缘贴住工作区边界后继续拖拽：位置钉在边界、尺寸按鼠标意图继续
  // 变化（底 / 右缘移动），保证贴顶面板可通过拖顶部持续拉高，不回退尺寸
  if (x < s.waX) {
    x = s.waX;
  }
  if (y < s.waY) {
    y = s.waY;
  }

  // 先 setSize 后 setPosition：两调用均为按最终目标值的绝对赋值，顺序仅影响
  // 单帧中间态，实测该顺序无可见闪跳；取整避免亚像素抖动；不 await，避免阻塞输入
  const win = getCurrentWindow();
  win.setSize(new LogicalSize(Math.round(w), Math.round(h))).catch(() => {
    /* 纯浏览器（npm run dev）无 Tauri IPC：静默 */
  });
  if (dir.includes("North") || dir.includes("West")) {
    win
      .setPosition(new LogicalPosition(Math.round(x), Math.round(y)))
      .catch(() => {
        /* 纯浏览器（npm run dev）无 Tauri IPC：静默 */
      });
  }
}

/** JS resize 起点：pointerdown 同步锁定 pointer capture，再异步采集几何快照 */
const startJsResize = (
  dir: TauriResizeDirection,
  e: ReactPointerEvent<HTMLDivElement>,
) => {
  const target = e.currentTarget;
  const pointerId = e.pointerId;
  // 同步捕获：后续 move / up 拖出窗口边界仍派发到本热区元素
  target.setPointerCapture(pointerId);
  Promise.all([
    currentMonitor(),
    getCurrentWindow().outerSize(), // 物理像素
    getCurrentWindow().outerPosition(), // 物理像素
  ])
    .then(([monitor, size, pos]) => {
      if (!monitor) throw new Error("currentMonitor() returned null");
      const sf = monitor.scaleFactor || 1;
      const wa = logicalWorkArea(monitor);
      jsSession = {
        dir,
        pointerId,
        startX: e.clientX,
        startY: e.clientY,
        winX: pos.x / sf,
        winY: pos.y / sf,
        winW: size.width / sf,
        winH: size.height / sf,
        waX: wa.x,
        waY: wa.y,
        pendingX: null,
        pendingY: null,
        rafId: null,
      };
    })
    .catch(() => {
      // 快照采集失败（纯浏览器无 IPC 等）：释放捕获，退回原生路径兜底
      jsSession = null;
      try {
        if (target.hasPointerCapture(pointerId)) {
          target.releasePointerCapture(pointerId);
        }
      } catch {
        /* 捕获已随指针事件流结束自动释放 */
      }
      startResize(dir);
    });
};

/** JS resize 过程：只记最新坐标，rAF 节流到每动画帧至多应用一次几何 */
const onJsResizeMove = (e: ReactPointerEvent<HTMLDivElement>) => {
  const s = jsSession;
  if (!s || e.pointerId !== s.pointerId) return;
  e.preventDefault();
  s.pendingX = e.clientX;
  s.pendingY = e.clientY;
  if (s.rafId === null) {
    s.rafId = window.requestAnimationFrame(() => {
      s.rafId = null;
      applyJsResizeGeometry(s);
    });
  }
};

/** JS resize 终点：复位会话、取消未执行的 rAF，并把实际终值落盘 */
const onJsResizeEnd = (e: ReactPointerEvent<HTMLDivElement>) => {
  const s = jsSession;
  if (!s || e.pointerId !== s.pointerId) return;
  jsSession = null;
  if (s.rafId !== null) {
    window.cancelAnimationFrame(s.rafId);
    s.rafId = null;
  }
  // 落盘终值：读实际 outerSize（防 clamp 差异）→ pxToPct → clamp → 落盘 → 广播
  // （up 时浏览器自动释放 pointer capture，无需显式释放）
  Promise.all([currentWinSizePct(), currentMonitor()])
    .then(([pct, monitor]) => {
      if (pct && monitor) persistPct(pct, monitor);
    })
    .catch(() => {
      /* 静默：落盘失败不影响本次拖拽结果 */
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
        // 400ms 防抖：拖拽期间连续触发不断重置计时，静止后落盘。
        // 不设「用户会话」门槛：系统热区拖拽时 mouseDown 不进 WebView，
        // 必须由此通道落盘；程序化 set_size 的三处来源防抖后稳定值均等于
        // 应存值（见文件头防误存论证）。会话终点路径会用最终尺寸覆盖中间值
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
      // 组件卸载时中断进行中的 JS resize 会话（capture 随元素移除自动释放）
      if (jsSession && jsSession.rafId !== null) {
        window.cancelAnimationFrame(jsSession.rafId);
      }
      jsSession = null;
    };
  }, []);

  return (
    // 全 view 共用的边缘热区层：fixed 相对 #root（transform 缩放容器）定位，
    // 厚度经 calc(… / --ui-scale) 补偿保持视觉恒定；z-60 高于现有 z-50 浮层；
    // 容器不拦截事件
    <div className="pointer-events-none fixed inset-0 z-60">
      {HANDLES.map((h) => (
        <div
          key={h.dir}
          className="pointer-events-auto absolute"
          style={h.style}
          onPointerDown={(e) => {
            if (e.button !== 0) return;
            e.preventDefault();
            // 平台分流：Win / macOS 走 JS 会话，其余走原生 startResizeDragging
            if (USE_JS_RESIZE) startJsResize(h.dir, e);
            else startResize(h.dir);
          }}
          onPointerMove={onJsResizeMove}
          onPointerUp={onJsResizeEnd}
          onPointerCancel={onJsResizeEnd}
        />
      ))}
    </div>
  );
}

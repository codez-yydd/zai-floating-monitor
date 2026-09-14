/**
 * 会话悬浮窗自绘拖拽热区（零 React 版，移植主面板 ResizeHandles.tsx）。
 *
 * 宿主壳 session-hud-main.ts 与宠物窗一样是零 React 薄壳（不引入 panel 的
 * 组件体系），无法复用主面板的 React 组件，本模块用原生 DOM 复刻同款方案：
 * - 7 向隐形热区（上/左边 8px + 下边 14px + 四角 14px 绝对定位铺满窗口边缘；
 *   纯 East 右缘热区已删除，见 DIRECTIONS 注释）：容器
 *   pointer-events: none、热区 pointer-events: auto、z 最高（几何/厚度/
 *   cursor 样式全在 session-hud.html 的 #hud-resize-layer .hud-rz-* 规则）；
 * - Windows / macOS 走 JS resize 会话：pointerdown 采集窗口几何快照
 *   （outerSize / outerPosition 物理 ÷ scaleFactor 换逻辑值）+ setPointerCapture
 *   锁定事件流，pointermove 按 8 向增量 setSize / setPosition 逻辑值，rAF
 *   节流到每动画帧至多一次；无系统消息模拟、无跨进程竞态（tao 在
 *   undecorated Windows 窗口上的系统热区不生效，startResizeDragging 又是
 *   异步模拟系统消息，详见 ResizeHandles.tsx 文件头）；
 * - Windows / macOS 上顶/左方向（North / West 及相关角）例外：优先走
 *   原生 startResizeDragging（V2 抖动修复）。这些方向每帧要同时改变位置
 *   与尺寸，JS 会话的 setSize + setPosition 是两个独立异步 IPC，非原子——
 *   两调用间的帧间空隙会形成可见中间态。原生路径由系统单消息原子处理
 *   pos + size，根治该竞态；其"偶发拖不动"失败模式（WebView2 竞态，见
 *   ResizeHandles.tsx 文件头）可接受——用户松手重按即可。同步失败
 *   （promise reject）时回退 JS 会话兜底；纯 South / East / SouthEast 方向
 *   只改尺寸，使用带最新目标合并的串行 JS 会话；
 * - 其余平台（Linux、识别失败）与快照采集失败时回退原生 startResizeDragging。
 *
 * 与 Rust 侧"用户调整中"标志的协作（防打架）：自适应高度模式下 poll_db
 * 每拍按 hud_height 公式 set_size，用户拖拽高度期间（pointerdown → up，
 * 秒级）程序侧 set_size 会把高度拉回自适应值（闪跳/拖不动）。因此会话开始
 * 即经 session_hud_resize_begin 置位 Rust 原子标志（session_hud.rs
 * HUD_USER_RESIZING），poll_db 在此期间跳过 set_size；pointerup / cancel
 * （含原生兜底路径的 promise settle）经 session_hud_resize_end 清标志并
 * 触发一次立即落盘核校（读实际窗口尺寸，绕开节流通道）。落盘仍有
 * Resized 挂点兜底（同尺寸回声由 LAST_SIZE 排除）。
 *
 * 尺寸 clamp 常量对齐来源（session_hud.rs）：最小值 = HUD_MIN_WIDTH /
 * HUD_MIN_HEIGHT（= 建窗 min_inner_size，悬浮窗无边框 outer == inner）；
 * 上限 = HUD_MAX_WIDTH / HUD_MAX_HEIGHT（落盘 clamp，防脏值与极端拖拽）。
 * 工作区边界 clamp 只约束 N / W 向（顶 / 左缘贴住工作区边界后继续拖拽时
 * 位置钉住、尺寸按鼠标意图继续变化，与主面板实现逐行同款）。
 */

import { invoke } from "@tauri-apps/api/core";
import {
  LogicalPosition,
  LogicalSize,
  currentMonitor,
  getCurrentWindow,
} from "@tauri-apps/api/window";
import { logicalWorkArea } from "./windowSize";

/** 热区层容器 id（幂等安装守卫 + session-hud.html 样式挂钩） */
const LAYER_ID = "hud-resize-layer";
/** 热区公共 class（与 session-hud.html 的 .hud-rz-* 规则配合） */
const ZONE_CLASS = "hud-rz";

/** 窗口最小逻辑尺寸（与 session_hud.rs 的 HUD_MIN_WIDTH / HUD_MIN_HEIGHT
 *  及建窗 min_inner_size 一致） */
const MIN_PX = { w: 260, h: 160 };
/** 窗口最大逻辑尺寸（与 session_hud.rs 的 HUD_MAX_WIDTH / HUD_MAX_HEIGHT
 *  落盘 clamp 一致） */
const MAX_PX = { w: 4096, h: 4096 };

/** 是否 Windows（与 ResizeHandles / windowSize.ts 同款 UA 判断风格） */
const isWindows =
  typeof navigator !== "undefined" && /windows/i.test(navigator.userAgent);
/** 是否 macOS：platform 优先（WebView 标准字段），缺失时退回 UA */
const isMac =
  typeof navigator !== "undefined" &&
  /mac/i.test(navigator.platform || navigator.userAgent);
/** JS resize 会话路径开关：明确识别为 Windows / macOS 才启用；其余平台
 *  （Linux、识别失败）一律走原生 startResizeDragging，零回归 */
const USE_JS_RESIZE = isWindows || isMac;

/** 顶/左方向（North / West 及四个相关角）需要同时改位置与尺寸。
 * Win / macOS 上优先使用系统原子 resize，避免 setSize + setPosition 两个
 * IPC 在同一帧形成可见中间态。 */
function isAnchoredDir(dir: ResizeDirection): boolean {
  return dir.includes("North") || dir.includes("West");
}

/** Tauri ResizeDirection 联合（@tauri-apps/api 2.11.1 未导出该类型，本地等价
 *  声明；East 仍属该系统联合，但本模块不再使用——见 DIRECTIONS 注释） */
type ResizeDirection =
  | "North"
  | "South"
  | "East"
  | "West"
  | "NorthEast"
  | "NorthWest"
  | "SouthEast"
  | "SouthWest";

/** 七个热区方向：三边在前、四角在后（角元素后插入，天然盖在边交叠处优先
 *  命中）；cls 与 session-hud.html 的 .hud-rz-* 规则一一对应。
 *
 *  纯 East（右缘 8px）热区已删除：右缘 4px 让位给 #hud-list 的自定义滚动条
 *  （session-hud.html 的 #hud-list::-webkit-scrollbar width: 4px），东向热区
 *  铺在窗口最右侧会整个盖住滚动条 —— 滚轮以外只能靠拖动滚动条滚动列表，
 *  热区拦截后滚动条拖不动。宽度调整仍可用：West 边（左缘向右拖）与
 *  NE / SE 四角（右缘在角内 14px 窄带，不及滚动条整列）。 */
const DIRECTIONS: { dir: ResizeDirection; cls: string }[] = [
  { dir: "North", cls: "hud-rz-n" },
  { dir: "South", cls: "hud-rz-s" },
  { dir: "West", cls: "hud-rz-w" },
  { dir: "NorthWest", cls: "hud-rz-nw" },
  { dir: "NorthEast", cls: "hud-rz-ne" },
  { dir: "SouthWest", cls: "hud-rz-sw" },
  { dir: "SouthEast", cls: "hud-rz-se" },
];

/** 取当前窗口句柄；纯浏览器（npm run dev 无 Tauri 注入）时返回 null（静默） */
function currentWin(): ReturnType<typeof getCurrentWindow> | null {
  try {
    return getCurrentWindow();
  } catch {
    return null;
  }
}

type ResizeGeometry = {
  width: number;
  height: number;
  x: number;
  y: number;
  move: boolean;
};

/** JS resize 会话快照：pointerdown 一次性采集；全程以「快照 + 最新指针增量」
 *  做绝对赋值计算，每帧目标值互不依赖，乱序到达也不会累积误差 */
type HudResizeSession = {
  dir: ResizeDirection;
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
  /** 最新待提交的窗口几何；新指针到来时覆盖旧目标，不堆积 IPC */
  desiredGeometry: ResizeGeometry | null;
  /** 当前串行写入链；结束时等待它完成，保证最终尺寸再落盘 */
  flushPromise: Promise<void> | null;
  writing: boolean;
};

/** 当前 JS resize 会话（同一时刻至多一个），null 表示空闲 */
let session: HudResizeSession | null = null;

/** 按方向增量计算窗口几何（始终从拖前快照计算，避免误差累积） */
function calculateGeometry(s: HudResizeSession): ResizeGeometry | null {
  if (s.pendingX === null || s.pendingY === null) return null;
  const dx = s.pendingX - s.startX;
  const dy = s.pendingY - s.startY;
  const dir = s.dir;
  let w = s.winW;
  let h = s.winH;
  let x = s.winX;
  let y = s.winY;

  // 8 向增量：E / S 加增量、W / N 减增量（W / N 拖大时窗口反向扩张、边缘外移）
  if (dir.includes("East")) w = s.winW + dx;
  if (dir.includes("West")) w = s.winW - dx;
  if (dir.includes("South")) h = s.winH + dy;
  if (dir.includes("North")) h = s.winH - dy;

  // clamp 到最小 / 最大尺寸（与 session_hud.rs 常量一致，见文件头）
  w = Math.min(Math.max(w, MIN_PX.w), MAX_PX.w);
  h = Math.min(Math.max(h, MIN_PX.h), MAX_PX.h);

  // N / W 联动 position，保持对边（S / E 边）不动
  if (dir.includes("West")) x = s.winX + (s.winW - w);
  if (dir.includes("North")) y = s.winY + (s.winH - h);

  // 工作区边界 clamp（仅 N / W 相关）：顶 / 左缘不许越过工作区上 / 左沿。
  // 顶 / 左缘贴住工作区边界后继续拖拽：位置钉在边界、尺寸按鼠标意图继续
  // 变化（底 / 右缘移动），不回退尺寸
  if (x < s.waX) {
    x = s.waX;
  }
  if (y < s.waY) {
    y = s.waY;
  }

  return {
    width: Math.round(w),
    height: Math.round(h),
    x: Math.round(x),
    y: Math.round(y),
    move: isAnchoredDir(dir),
  };
}

/** 串行提交最新几何。窗口 API 前一笔完成前不发下一笔，且中途只保留
 * 最新目标，避免 IPC 乱序把窗口短暂拉回旧尺寸造成抖动。 */
async function flushGeometry(s: HudResizeSession): Promise<void> {
  const win = currentWin();
  if (!win) {
    s.desiredGeometry = null;
    return;
  }
  while (s.desiredGeometry !== null) {
    const geometry = s.desiredGeometry;
    s.desiredGeometry = null;
    try {
      await win.setSize(new LogicalSize(geometry.width, geometry.height));
      // 有更新目标时跳过旧目标的位置写入，避免旧位置在最新几何前闪一下。
      if (geometry.move && s.desiredGeometry === null) {
        await win.setPosition(new LogicalPosition(geometry.x, geometry.y));
      }
    } catch {
      /* 纯浏览器 / ACL 拒绝：丢弃本笔，保留循环中可能到来的最新目标 */
    }
  }
}

function enqueueGeometry(s: HudResizeSession): void {
  const geometry = calculateGeometry(s);
  if (!geometry) return;
  s.desiredGeometry = geometry;
  if (s.writing) return;
  s.writing = true;
  const flush = flushGeometry(s).finally(() => {
    s.writing = false;
  });
  s.flushPromise = flush;
  void flush;
}

/** 会话开始：置位 Rust 侧"用户调整中"标志（poll_db 暂停自适应 set_size，
 *  防拖拽期间高度被程序拉回）。返回的 promise 会被 resize 快照等待，
 *  保证第一笔 setSize 不会跑在 Rust 标志置位之前。 */
function beginAdjust(): Promise<void> {
  return invoke("session_hud_resize_begin").then(
    () => undefined,
    () => {
      /* 纯浏览器无 IPC：静默（本次拖拽仍可用，只是无防打架保护） */
    },
  );
}

/** 会话结束：清标志 + 触发一次立即落盘核校（Rust 侧读实际窗口尺寸落盘，
 *  绕过节流通道可能存的中间值）；fire-and-forget 不阻塞 pointerup */
function finishAdjust(): Promise<void> {
  return invoke("session_hud_resize_end").then(
    () => undefined,
    () => {
      /* 纯浏览器无 IPC：静默（Resized 挂点仍是落盘兜底） */
    },
  );
}

/** 原生 resize 兜底（非 Win / macOS 平台或快照采集失败）：交系统
 *  startResizeDragging；promise settle（拖拽会话终点）后同款收尾 */
function startNativeResize(dir: ResizeDirection, ready: Promise<void> = Promise.resolve()): void {
  ready.then(() => {
    const win = currentWin();
    if (!win) {
      void finishAdjust();
      return;
    }
    win
      .startResizeDragging(dir)
      .then(() => finishAdjust())
      .catch(() => finishAdjust());
  });
}

/** 原生 resize 优先（Win / macOS 的顶/左方向，V2 抖动修复主路径）：系统级
 *  单消息原子处理 pos + size，根治 JS 会话 setSize + setPosition 双 IPC
 *  非原子在顶/左方向的抖动 / 位移（见文件头）。promise settle（拖拽会话
 *  终点）后与 JS 会话完全同款收尾（清标志 + 落盘核校）；同步失败
 *  （promise reject：无 IPC / 被 ACL 拒绝等）时回退 JS 会话兜底。
 *  注意 WebView2 竞态的"偶发拖不动"失败模式下 promise 正常 resolve，
 *  不会触发回退（属可接受失败：用户松手重按即可，标志由 settle 清除，
 *  落盘路径不受影响） */
function startNativeResizePreferred(
  dir: ResizeDirection,
  e: PointerEvent,
  ready: Promise<void>,
): void {
  // 同步上下文预取热区元素：DOM 规范里 e.currentTarget 仅在事件传播期间
  // 有值，异步 reject 回调中已被置 null——不预取的话 JS 兜底会误判"无
  // target"直接收尾，回退失效
  const target = e.currentTarget as HTMLElement | null;
  ready.then(() => {
    const win = currentWin();
    if (!win) {
      void finishAdjust();
      return;
    }
    win.startResizeDragging(dir).then(
      () => finishAdjust(),
      () => {
        // 原生调用同步失败：回退 JS 会话兜底（此时已离开 pointerdown 的
        // 同步上下文，capture 可能设不上——startJsResizeAt 内部容忍）
        startJsResizeAt(dir, e, target, Promise.resolve());
      },
    );
  });
}

/** JS resize 起点（pointerdown 事件处理器直接调用）：同步上下文中
 *  currentTarget 有效，直接透传 */
function startJsResize(dir: ResizeDirection, e: PointerEvent, ready: Promise<void>): void {
  startJsResizeAt(dir, e, e.currentTarget as HTMLElement | null, ready);
}

/** JS resize 起点实现（target 由调用方解析：同步路径取 e.currentTarget，
 *  异步回退路径用预取值——见 startNativeResizePreferred 注释） */
function startJsResizeAt(
  dir: ResizeDirection,
  e: PointerEvent,
  target: HTMLElement | null,
  ready: Promise<void>,
): void {
  const pointerId = e.pointerId;
  const win = currentWin();
  if (!target || !win) {
    // 异常环境（无 Tauri 注入）：与 beginAdjust 配对收尾，防"用户调整中"
    // 标志残留（纯浏览器下 invoke 本身也会失败，双保险）
    finishAdjust();
    return;
  }
  // 同步捕获：后续 move / up 拖出窗口边界仍派发到本热区元素。异步回退
  // 路径（原生 N 向失败转 JS 兜底）调用本函数时已过 pointerdown 同步
  // 上下文，capture 可能抛 InvalidStateError——容忍失败（无捕获时事件
  // 仅在指针位于热区内派发，拖出窗口边界即失效，兜底路径的降级行为）
  try {
    target.setPointerCapture(pointerId);
  } catch {
    /* 见上注释 */
  }
  Promise.all([
    ready,
    currentMonitor(),
    win.outerSize(), // 物理像素
    win.outerPosition(), // 物理像素
  ])
    .then(([, monitor, size, pos]) => {
      if (!monitor) throw new Error("currentMonitor() returned null");
      const sf = monitor.scaleFactor || 1;
      const wa = logicalWorkArea(monitor);
      session = {
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
        desiredGeometry: null,
        flushPromise: null,
        writing: false,
      };
    })
    .catch(() => {
      // 快照采集失败（无 IPC / 被 ACL 拒绝等）：释放捕获，退回原生路径兜底
      session = null;
      try {
        if (target.hasPointerCapture(pointerId)) {
          target.releasePointerCapture(pointerId);
        }
      } catch {
        /* 捕获已随指针事件流结束自动释放 */
      }
      startNativeResize(dir, Promise.resolve());
    });
}

/** JS resize 过程：只记最新坐标，rAF 节流到每动画帧至多应用一次几何 */
function onResizeMove(e: PointerEvent): void {
  const s = session;
  if (!s || e.pointerId !== s.pointerId) return;
  e.preventDefault();
  s.pendingX = e.clientX;
  s.pendingY = e.clientY;
  if (s.rafId === null) {
    s.rafId = window.requestAnimationFrame(() => {
      s.rafId = null;
      enqueueGeometry(s);
    });
  }
}

/** JS resize 终点：先冲刷最后一笔指针坐标，再复位会话、取消未执行的 rAF，
 * 并通知 Rust 清标志 + 落盘核校。否则用户在下一帧到来前松手时，最后一段
 * 高度增量会被 cancelAnimationFrame 丢掉，表现为下边缘拖到某处却少走一截。 */
function onResizeEnd(e: PointerEvent): void {
  const s = session;
  if (!s || e.pointerId !== s.pointerId) return;
  if (s.rafId !== null) {
    window.cancelAnimationFrame(s.rafId);
    s.rafId = null;
  }
  // pointercancel 的坐标在部分 WebView 中可能回落到 (0, 0)，只在正常
  // pointerup 时用终点坐标覆盖；取消事件则冲刷已记录的最后一笔 move。
  if (e.type === "pointerup") {
    s.pendingX = e.clientX;
    s.pendingY = e.clientY;
  }
  enqueueGeometry(s);
  session = null;
  // up / cancel 时浏览器自动释放 pointer capture，无需显式释放
  void (s.flushPromise ?? Promise.resolve()).then(() => finishAdjust());
}

/** 安装隐形热区层（零 React 原生 DOM；幂等：重复调用不产生第二层）。
 *  在宿主壳 main() 起始调用，页面存在期间常驻（窗口销毁时随页面一并
 *  回收，无需卸载路径）。 */
export function installHudResizeHandles(): void {
  if (document.getElementById(LAYER_ID)) return;
  const layer = document.createElement("div");
  layer.id = LAYER_ID;
  for (const { dir, cls } of DIRECTIONS) {
    const zone = document.createElement("div");
    zone.className = `${ZONE_CLASS} ${cls}`;
    zone.addEventListener("pointerdown", (e) => {
      if (e.button !== 0) return;
      e.preventDefault();
      // 会话开始即置位"用户调整中"（原生路径同样需要，见文件头）；
      // 三条路径的终点都会 finishAdjust()（清标志 + 落盘核校）
      const ready = beginAdjust();
      // 平台 + 方向分流（V2）：Win / macOS 的顶/左方向优先原生原子路径
      //（根治位置 + 尺寸双 IPC 抖动，reject 回退 JS 兜底）；其余方向走
      // 串行 JS 会话；其他平台走原生 startResizeDragging
      if (USE_JS_RESIZE) {
        if (isAnchoredDir(dir)) startNativeResizePreferred(dir, e, ready);
        else startJsResize(dir, e, ready);
      } else {
        startNativeResize(dir, ready);
      }
    });
    zone.addEventListener("pointermove", onResizeMove);
    zone.addEventListener("pointerup", onResizeEnd);
    zone.addEventListener("pointercancel", onResizeEnd);
    layer.append(zone);
  }
  // 放进 HUD 根节点而不是 body：根节点有明确的 100% 视口和相对定位，
  // 数据列表变为可滚动/固定高度后，热区仍始终贴在实际窗口边缘。
  // 作为根节点最后一个绝对定位子项，z-index 也不会被 WebView 的内容层
  // 或滚动条 stacking context 抢走。
  (document.getElementById("hud-root") ?? document.body).append(layer);
}

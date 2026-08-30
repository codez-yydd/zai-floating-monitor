/**
 * pet-core.js 状态机单元测试（Node 直跑，无测试框架依赖）：
 *
 *   node scripts/test-pet-core.mjs
 *
 * 测试对象为 V5 抽出的模块级纯函数 ZBarPet.decideState（状态机判定的
 * 无状态内核——预判触发与超时、working 迟滞保持与超时、celebrating
 * 优先级、pu 缺失兼容、心跳陈旧短路；V6 新增 tool_running 触发与超时、
 * failed 沮丧窗口、迟滞跨工作态语义；V9 将原 working 三场景细分为
 * thinking（runs 活跃但 out 不增长）/walking（pu 预判、迟滞保持），
 * working 降为缺键形象的回退帧目标）；DOM 依赖部分按输入侧测试：
 * 用桩 DOM 创建真实实例并 feed 带 pu/ta/fe 与不带的旧数据快照，验证
 * 喂入路径（解析与容错、轮完成成败互斥分支）不抛错。V8 起形象渲染
 * 收敛为 customAsset-only（Petdex 图集），实例创建经 customAsset 桩
 * （meta + 假 dataUri + 桩 Image 同步 onload）驱动真实图集渲染路径；
 * 「无资产不渲染（空态）」与「资产就位热切换」同步断言。视觉渲染效果
 * 不在本测试范围（由 inject.rs 契约扫描与 pets.rs 单测覆盖）。
 */
import { readFileSync } from "node:fs";
import { fileURLToPath } from "node:url";
import { dirname, join } from "node:path";

const root = dirname(dirname(fileURLToPath(import.meta.url)));
const source = readFileSync(join(root, "public", "pet-core.js"), "utf8");
/* 行尾归一形态（源文件行尾格式随仓库配置不定，多行契约断言统一按 LF 匹配） */
const srcLf = source.replace(/\r\n/g, "\n");

/* 加载核心：IIFE 只依赖 window 全局（document/performance 仅在实例
 * 创建后的渲染路径使用，纯判定不触碰） */
const windowStub = {};
new Function("window", source)(windowStub);
const { ZBarPet } = windowStub;

if (!ZBarPet || typeof ZBarPet.create !== "function") {
  throw new Error("window.ZBarPet 工厂缺失");
}
if (typeof ZBarPet.decideState !== "function") {
  throw new Error("ZBarPet.decideState 纯判定函数缺失（V5 单测入口）");
}

const decide = ZBarPet.decideState;
let failed = 0;
let passed = 0;
function check(name, actual, expected) {
  if (actual === expected) {
    passed += 1;
  } else {
    failed += 1;
    console.error(`FAIL ${name}: 期望 ${expected}，实际 ${actual}`);
  }
}

/* 侧写基线：数据有效、心跳 5 秒前（新鲜）、la 5 秒前（闲置档内）、
 * 无 runs / 无庆祝 / 无沮丧 / 无工具 / 无预判 / 从未工作。时间基准取
 * 10^9 量级毫秒 */
const NOW = 1_000_005_000;
const ACT = NOW - 5_000;
function side(overrides = {}) {
  return {
    hasData: true,
    lastHb: NOW - 5_000,
    runsActive: false,
    outGrowing: false,
    celebrateUntil: 0,
    failedUntil: 0,
    toolActive: false,
    pending: false,
    lastWorkT: 0,
    lastActivity: ACT,
    ...overrides,
  };
}

/* ---- 既有判定回归（V4 行为基线） ---- */
check("无数据 → sleeping", decide(NOW, side({ hasData: false })), "sleeping");
check(
  "心跳陈旧 → sleeping（优先级最高）",
  decide(NOW, side({ lastHb: NOW - 60_000, runsActive: true })),
  "sleeping"
);
check(
  "心跳陈旧且有预判 → 仍 sleeping",
  decide(NOW, side({ lastHb: NOW - 60_000, pending: true })),
  "sleeping"
);
check("runs 活跃 + out 增长 → typing", decide(NOW, side({ runsActive: true, outGrowing: true })), "typing");
check(
  "runs 活跃 + out 停滞 → thinking（V9：模型思考/规划，原 working 场景细分）",
  decide(NOW, side({ runsActive: true })),
  "thinking"
);
check(
  "runs 活跃 dominance：迟滞/预判不干扰",
  decide(NOW, side({ runsActive: true, outGrowing: true, pending: true, lastWorkT: NOW })),
  "typing"
);
check("runs 空 + la 新鲜 → idle（pu 缺失基线）", decide(NOW, side()), "idle");
check("runs 空 + la 超窗 → sleeping", decide(NOW, side({ lastActivity: NOW - 120_000 })), "sleeping");

/* ---- V5 预判：pu 命中 → walking（V9：动身去干活，原输出 working） ---- */
check(
  "预判触发：runs 空 + pending → walking（优先于 idle/sleeping）",
  decide(NOW, side({ pending: true })),
  "walking"
);
check(
  "预判触发：la 超窗也不入睡（pu 即活动信号）",
  decide(NOW, side({ pending: true, lastActivity: NOW - 120_000 })),
  "walking"
);
check(
  "预判超时（pending 已按窗口预计算为 false）→ 回落 idle",
  decide(NOW, side({ pending: false })),
  "idle"
);
check(
  "pu 缺失兼容：pending=false + lastWorkT=0 → 行为同 V4",
  decide(NOW, side({ pending: false, lastActivity: NOW - 120_000 })),
  "sleeping"
);

/* ---- V5 迟滞：工作信号消失后保持 walking（V9：踱步等待下一轮，原输出 working） ---- */
check(
  "迟滞保持：lastWorkT 10 秒前 → walking",
  decide(NOW, side({ lastWorkT: NOW - 10_000 })),
  "walking"
);
check(
  "迟滞保持：la 超窗也在窗口内维持 walking",
  decide(NOW, side({ lastWorkT: NOW - 10_000, lastActivity: NOW - 120_000 })),
  "walking"
);
check(
  "迟滞超时：lastWorkT 50 秒前 → 回落 idle",
  decide(NOW, side({ lastWorkT: NOW - 50_000 })),
  "idle"
);
check(
  "迟滞超时且 la 超窗 → sleeping",
  decide(NOW, side({ lastWorkT: NOW - 50_000, lastActivity: NOW - 120_000 })),
  "sleeping"
);
check(
  "迟滞不放大：lastWorkT=0（从未工作）→ idle",
  decide(NOW, side({ lastWorkT: 0 })),
  "idle"
);

/* ---- celebrating 优先级：高于预判与迟滞 ---- */
check(
  "celebrating 优先于预判",
  decide(NOW, side({ celebrateUntil: NOW + 1_000, pending: true })),
  "celebrating"
);
check(
  "celebrating 优先于迟滞",
  decide(NOW, side({ celebrateUntil: NOW + 1_000, lastWorkT: NOW })),
  "celebrating"
);
check(
  "庆祝结束（celebrateUntil 已过）→ 迟滞窗口内回 walking",
  decide(NOW, side({ celebrateUntil: NOW - 1, lastWorkT: NOW - 10_000 })),
  "walking"
);
check(
  "庆祝结束且无迟滞 → 回落 idle",
  decide(NOW, side({ celebrateUntil: NOW - 1 })),
  "idle"
);

/* ---- V6 常量存在性（源码扫描：判定窗口的核心常量口径） ---- */
check(
  "TOOL_ACTIVE_MS = 30000 常量存在",
  source.includes("TOOL_ACTIVE_MS = 30000"),
  true
);
check("FAILED_MS = 3000 常量存在", source.includes("FAILED_MS = 3000"), true);

/* ---- V6 failed：fe 驱动的沮丧窗口（优先级最高，仅次于心跳陈旧短路） ---- */
check(
  "failed 触发：failedUntil 在窗 → failed（盖过 runs 活跃）",
  decide(NOW, side({ failedUntil: NOW + 1_000, runsActive: true, outGrowing: true })),
  "failed"
);
check(
  "failed 触发：盖过 celebrating（互斥由 feed 成败分支保证，顺序兜底）",
  decide(NOW, side({ failedUntil: NOW + 1_000, celebrateUntil: NOW + 1_000 })),
  "failed"
);
check(
  "failed 触发：盖过预判与迟滞",
  decide(NOW, side({ failedUntil: NOW + 1_000, pending: true, lastWorkT: NOW })),
  "failed"
);
check(
  "failed 超时（failedUntil 已过）→ 回落 idle",
  decide(NOW, side({ failedUntil: NOW - 1 })),
  "idle"
);
check(
  "failed 超时且迟滞窗口内 → 迟滞接管 walking（工作类回落语义）",
  decide(NOW, side({ failedUntil: NOW - 1, lastWorkT: NOW - 10_000 })),
  "walking"
);
check(
  "心跳陈旧 + failedUntil 在窗 → 仍 sleeping（数据源不在优先于一切）",
  decide(NOW, side({ failedUntil: NOW + 1_000, lastHb: NOW - 60_000 })),
  "sleeping"
);

/* ---- V6 tool_running：ta 驱动的工具执行态（优先于 typing/working） ---- */
check(
  "tool_running 触发：runs 活跃 + 工具活跃（out 停滞）",
  decide(NOW, side({ runsActive: true, toolActive: true })),
  "tool_running"
);
check(
  "typing 让位 tool_running：out 增长时工具执行观感优先（工具执行期 out "
    + "通常不增长，typing 自然让位）",
  decide(NOW, side({ runsActive: true, toolActive: true, outGrowing: true })),
  "tool_running"
);
check(
  "tool_running 触发：runs 空 + pu 预判命中（首笔模型请求未落库、首工具已开跑）",
  decide(NOW, side({ pending: true, toolActive: true })),
  "tool_running"
);
check(
  "tool_running 不触发：工具活跃但 runs 空且无预判（陈旧/异常防御，"
    + "迟滞窗口外回落 idle）",
  decide(NOW, side({ toolActive: true })),
  "idle"
);
check(
  "tool_running 不触发但迟滞在窗：从工作类状态回落的迟滞保持 walking",
  decide(NOW, side({ toolActive: true, lastWorkT: NOW - 10_000 })),
  "walking"
);
check(
  "ta 超时（toolActive 已按 TOOL_ACTIVE_MS 预计算为 false）→ thinking 兜底",
  decide(NOW, side({ runsActive: true, toolActive: false })),
  "thinking"
);
check(
  "tool_running dominance：runs 活跃时优先于预判与迟滞分支",
  decide(
    NOW,
    side({ runsActive: true, toolActive: true, outGrowing: true, pending: true, lastWorkT: NOW })
  ),
  "tool_running"
);
check(
  "庆祝期间 pu+工具开跑不打断庆祝（预判语义与 V5 一致；runs 出现才打断）",
  decide(NOW, side({ celebrateUntil: NOW + 1_000, pending: true, toolActive: true })),
  "celebrating"
);
check(
  "庆祝期间 runs 活跃 + 工具开跑 → tool_running（新轮打断庆祝，V5 语义）",
  decide(NOW, side({ celebrateUntil: NOW + 1_000, runsActive: true, toolActive: true })),
  "tool_running"
);

/* ---- V6 源码级契约：缺键回退与迟滞跨工作态（inject.rs 同款扫描） ---- */
check(
  "缺键回退：tool_running → typing 帧（旧五状态 pet.json 兼容）",
  source.includes('tool_running: "typing"'),
  true
);
check(
  "缺键回退：failed → sleeping 帧（旧五状态 pet.json 兼容）",
  source.includes('failed: "sleeping"'),
  true
);
check(
  "迟滞基准 lastWorkT 覆盖 tool_running（工作类状态统一推进）",
  source.includes('st === "tool_running"'),
  true
);

/* ---- V9 源码级契约：细分判定与缺键回退（inject.rs 同款扫描） ---- */
check(
  "V9：runs 活跃不增长应输出 thinking（不再输出 working）",
  source.includes('return s.outGrowing ? "typing" : "thinking";'),
  true
);
check(
  "V9：预判命中应输出 walking",
  srcLf.includes('if (s.pending) {\n      if (s.toolActive) return "tool_running";\n      return "walking";'),
  true
);
check(
  "V9：迟滞保持应输出 walking",
  srcLf.includes(
    'if (s.lastWorkT > 0 && now - s.lastWorkT < WORKING_LINGER_MS) {\n      return "walking";'
  ),
  true
);
check(
  "缺键回退：thinking → working 帧（未随细分映射升级的形象，观感同 V8）",
  source.includes('thinking: "working"'),
  true
);
check(
  "缺键回退：walking → working 帧（未随细分映射升级的形象，观感同 V8）",
  source.includes('walking: "working"'),
  true
);
check(
  "迟滞基准 lastWorkT 覆盖 thinking/walking（V9 细分状态同为工作类）",
  source.includes('st === "thinking"') && source.includes('st === "walking"'),
  true
);

/* ---- V8 源码级契约：渲染收敛 customAsset-only ---- */
check(
  "V8：字符网格形象库 PET_STYLES 应已移除",
  source.includes("var PET_STYLES"),
  false
);
check(
  "V8：内建渲染回退 builtinRenderState 应已移除",
  source.includes("builtinRenderState"),
  false
);
check(
  "V9：版本头应为 ZBAR-THEME-V9",
  source.includes("ZBAR-THEME-V9"),
  true
);

/* ---- DOM 依赖部分：输入侧测试（feed 喂入路径不抛错 + V8 空态语义） ---- */
function fakeElement(tag) {
  if (tag === "canvas") {
    return {
      tagName: "CANVAS",
      width: 0,
      height: 0,
      style: { cssText: "", setProperty() {} },
      getContext() {
        return {
          clearRect() {},
          fillRect() {},
          drawImage() {},
          set fillStyle(v) {},
          set imageSmoothingEnabled(v) {},
        };
      },
    };
  }
  return {
    tagName: tag.toUpperCase(),
    style: {},
    setAttribute() {},
    appendChild() {},
    parentNode: null,
  };
}
const documentStub = { createElement: fakeElement };
/* 桩 Image：src 赋值同步触发 onload（图集立即就绪，动画循环可推进） */
class ImageStub {
  constructor() {
    this.onload = null;
    this.onerror = null;
  }
  set src(v) {
    if (this.onload) this.onload();
  }
}
globalThis.Image = ImageStub;
/* 挂载计数容器：验证空态不挂 DOM / 资产就位后挂载 */
function makeContainer() {
  return {
    querySelector: () => null,
    appended: 0,
    appendChild() {
      this.appended += 1;
    },
  };
}
/* customAsset 桩（V8 渲染必需）：智谱娘同款网格形态 + 假 dataUri
 *（V9：states 含细分键 thinking/walking，与升级后的内置 pet.json 一致；
 * 缺键形象的回退路径由上方源码级契约断言覆盖——CUSTOM_STATE_FALLBACK
 * 是实例内部闭包，桩 DOM 无法直接观测切帧目标） */
const STUB_ASSET = {
  meta: {
    id: "zhipu-z-niang",
    name: "智谱 Z 娘",
    format: "petdex-v2",
    cols: 8,
    rows: 11,
    frameW: 192,
    frameH: 208,
    image: "sheet.webp",
    states: {
      sleeping: { row: 6, frames: 6, frameMs: 800 },
      idle: { row: 0, frames: 6, frameMs: 450 },
      thinking: { row: 9, frames: 8, frameMs: 400 },
      typing: { row: 7, frames: 6, frameMs: [220, 150, 95] },
      walking: { row: 8, frames: 6, frameMs: 300 },
      working: { row: 8, frames: 6, frameMs: 300 },
      celebrating: { row: 4, frames: 5, frameMs: 160 },
    },
  },
  dataUri: "data:image/webp;base64,QUJD",
};

/* 核心经全局 document 创建元素（与浏览器宿主同路径），喂入前注入桩 */
globalThis.document = documentStub;

/* ---- V8 空态：无资产不渲染（实例存活、不挂 DOM、接口不炸） ---- */
const emptyContainer = makeContainer();
const emptyPet = ZBarPet.create(emptyContainer, { style: "custom:zhipu-z-niang", size: 64 });
check("V8 空态：无资产的 custom 实例应创建成功（存活）", !!emptyPet, true);
check("V8 空态：无资产不应挂任何 DOM", emptyContainer.appended, 0);
let emptyOk = true;
try {
  if (emptyPet) {
    emptyPet.feed({ v: 2, ts: NOW, la: NOW, turns: [], runs: [] });
    emptyPet.heartbeat(NOW);
    emptyPet.setParams({ size: 96 });
    emptyPet.setParams({ style: "custom:zhipu-z-niang", customAsset: STUB_ASSET });
  }
} catch (e) {
  emptyOk = false;
  console.error("FAIL V8 空态接口调用不应抛错:", e);
}
if (emptyOk) passed += 1;
else failed += 1;
/* 资产就位热切换：空态实例 setParams 传入资产 → 重建挂载（宿主侧保证
 * 资产就位后的同款路径） */
check(
  "V8 热切换：空态实例传入资产后应挂载 DOM",
  emptyPet ? emptyContainer.appended > 0 : false,
  true
);

/* ---- 带资产的真实渲染路径（feed 输入侧全覆盖） ---- */
const container = makeContainer();
const pet = ZBarPet.create(container, {
  style: "custom:zhipu-z-niang",
  size: 64,
  customAsset: STUB_ASSET,
});
check("V8 渲染：带资产的 custom 实例应创建并挂载", container.appended > 0, true);
let integrationOk = true;
try {
  if (pet) {
    /* 带 pu/ta/fe 的数据快照（数值态）、旧数据文件（无 pu/ta/fe）与
     * V6 成败互斥分支路径（turns 新增 + fe 新鲜 → failedUntil 置位）
     * 各喂一次 */
    pet.feed({
      v: 2, ts: NOW, la: NOW - 1000, pu: NOW - 2000,
      ta: NOW - 3000, fe: NOW - 4000, turns: [], runs: [],
    });
    /* 旧数据文件（V5 前形态：无 pu/ta/fe）→ 兼容不抛错 */
    pet.feed({ v: 2, ts: NOW + 1000, la: NOW, turns: [], runs: [] });
    /* 轮完成新增 + fe 刚刷新（失败轮）→ 沮丧互斥分支 */
    pet.feed({
      v: 2, ts: NOW + 2000, la: NOW + 1000, pu: null, ta: null, fe: NOW + 1000,
      turns: [{ turn: "turn_f", umid: "msg_f" }], runs: [],
    });
    /* 轮完成新增 + fe 停在旧值（成功轮不刷新 fe）→ 庆祝分支 */
    pet.feed({
      v: 2, ts: NOW + 4000, la: NOW + 3000, pu: null, ta: null, fe: NOW + 1000,
      turns: [{ turn: "turn_f", umid: "msg_f" }, { turn: "turn_ok", umid: "msg_ok" }],
      runs: [{ out: 10 }],
    });
    /* 工具活跃快照（ta 数值 + runs 活跃 → tool_running 输入路径） */
    pet.feed({
      v: 2, ts: NOW + 6000, la: NOW + 5000, pu: null, ta: NOW + 5000, fe: null,
      turns: [{ turn: "turn_ok", umid: "msg_ok" }], runs: [{ out: 20 }],
    });
    /* V9 思考快照（runs 活跃但 out 停滞 → thinking 输入路径） */
    pet.feed({
      v: 2, ts: NOW + 7000, la: NOW + 6500, pu: null, ta: null, fe: null,
      turns: [{ turn: "turn_ok", umid: "msg_ok" }], runs: [{ out: 20 }],
    });
    /* fe/ta 非法值（0/负数/字符串）归 0 不抛错 */
    pet.feed({
      v: 2, ts: NOW + 8000, la: NOW + 7000, pu: 0, ta: -1, fe: "x",
      turns: [], runs: [],
    });
    pet.heartbeat(NOW + 2000);
    pet.heartbeat(0); /* 非法心跳应被忽略（不抛错） */
    pet.feed(null); /* 非法数据应被忽略（不抛错） */
    pet.setParams({ size: 96 });
    /* 同 id 新资产对象（重复导入替换）→ 按身份重建不抛错 */
    pet.setParams({
      style: "custom:zhipu-z-niang",
      customAsset: { meta: STUB_ASSET.meta, dataUri: "data:image/webp;base64,WE5F" },
    });
    /* 未知内建残留值（cat）→ V8 语义为空态不渲染，不抛错不闪猫 */
    pet.setParams({ style: "cat" });
    check("V8 回退语义：cat 残留值应进入空态（不渲染）", container.appended >= 0, true);
    pet.destroy();
  }
} catch (e) {
  integrationOk = false;
  console.error("FAIL 喂入路径不应抛错:", e);
}
if (integrationOk) passed += 1;
else failed += 1;

/* 注：桩 DOM 覆盖 createElement/getContext/Image 等最小面，实例创建应
 * 走通并驱动真实的 feed 解析路径；渲染视觉效果不在断言范围 */
if (!pet) {
  failed += 1;
  console.error("FAIL 桩 DOM 下实例应创建成功（feed 输入侧路径的前提）");
}

/* ---- V9 迟滞行为级回归（实例级，防外层守卫被误删导致永久 walking） ----
 * 源码级断言只能证明 lastWorkT 推进列表含 walking，不能防
 * (runsActive || pending) 外层守卫被重构误删——若迟滞期的 walking 也
 * 回写 lastWorkT，45 秒窗口会被每次 tick 无限续期，宠物永久踱步。
 * 方案：mock Date.now 驱动假时钟（tick 每 1 秒真实触发一次，只消费
 * 假时刻），用记录版 canvas ctx 捕获 drawImage 的源行号（= 当前状态
 * 行：idle=0 / typing=7 / walking=8 / thinking=9 / sleeping=6），
 * 观测实例真实的闭包侧写输出。 */
const realDateNow = Date.now;
const T0 = 1_000_100_000;
let fakeNow = T0;
Date.now = () => fakeNow; /* 假时钟：feed 的 ts 消费与 tick 的判定共用 */
const rowsDrawn = []; /* 最近绘制行（drawFrame 经 drawImage 透出状态行） */
const recElement = (tag) => {
  const el = fakeElement(tag);
  if (tag === "canvas") {
    const origGetContext = el.getContext;
    el.getContext = function () {
      const ctx = origGetContext();
      const rec = Object.create(ctx); /* 原型链保留 clearRect/setter 等 */
      rec.drawImage = function (img, sx, sy) {
        rowsDrawn.push(Math.round(sy / 208)); /* STUB_ASSET.frameH=208 */
      };
      return rec;
    };
  }
  return el;
};
globalThis.document = { createElement: recElement };
const sleepReal = (ms) => new Promise((r) => setTimeout(r, ms));
const lagPet = ZBarPet.create(makeContainer(), {
  style: "custom:zhipu-z-niang",
  size: 64,
  customAsset: STUB_ASSET,
});
if (!lagPet) {
  failed += 1;
  console.error("FAIL 迟滞回归：实例应创建成功");
} else {
  /* 1. runs 活跃且 out 停滞（首帧 out=0 不增长）→ thinking，
   *    tick 推进迟滞基准 lastWorkT=T0+1s */
  lagPet.feed({
    v: 2, ts: T0, la: T0, pu: null, ta: null, fe: null,
    turns: [], runs: [{ out: 0 }],
  });
  fakeNow = T0 + 1_000;
  lagPet.heartbeat(T0 + 1_000);
  rowsDrawn.length = 0;
  await sleepReal(1_200); /* 真实等待下一次 tick（TICK_MS=1000） */
  check(
    "迟滞回归：runs 活跃不增长 → thinking（基准随真实工作信号推进）",
    rowsDrawn[rowsDrawn.length - 1],
    9
  );

  /* 2. runs 清空 → 迟滞窗口内（lastWorkT 距今 11s < 45s）→ walking */
  lagPet.feed({
    v: 2, ts: T0 + 2_000, la: T0 + 2_000, pu: null, ta: null, fe: null,
    turns: [], runs: [],
  });
  fakeNow = T0 + 12_000;
  lagPet.heartbeat(T0 + 12_000);
  rowsDrawn.length = 0;
  await sleepReal(1_200);
  check(
    "迟滞回归：runs 清空后窗口内 → walking（踱步等待下一轮）",
    rowsDrawn[rowsDrawn.length - 1],
    8
  );

  /* 3. 迟滞期再喂空快照 + 假时钟越窗（距今 47s > 45s）→ 应回落 idle。
   *    若守卫被误删（迟滞 walking 回写 lastWorkT=上拍时刻），距今仅
   *    36s < 45s → 仍 walking（记录停留行 8），断言失败——这正是要防
   *    的「永久踱步」回归 */
  lagPet.feed({
    v: 2, ts: T0 + 3_000, la: T0 + 3_000, pu: null, ta: null, fe: null,
    turns: [], runs: [],
  });
  fakeNow = T0 + 48_000;
  lagPet.heartbeat(T0 + 48_000); /* 保持心跳新鲜（防误判陈旧入沉睡） */
  rowsDrawn.length = 0;
  await sleepReal(1_200);
  check(
    "迟滞回归：越窗回落 idle（迟滞 walking 不回写 lastWorkT，防永久踱步）",
    rowsDrawn[rowsDrawn.length - 1],
    0
  );
  lagPet.destroy();
}
Date.now = realDateNow; /* 恢复真实时钟（后续统计与进程退出不受影响） */

console.log(`pet-core 状态机测试：${passed} 通过，${failed} 失败`);
process.exit(failed === 0 ? 0 : 1);

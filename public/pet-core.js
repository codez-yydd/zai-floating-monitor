// ============================================================
// ZBAR-THEME-V9
// ZBar 桌面像素宠物核心（宠物状态机 + Petdex 图集 canvas 渲染器）
// ============================================================
// 本文件是宠物核心的唯一真相源，两个宿主消费同一份代码：
//   - 皮肤注入版：inject.rs 的 PET_JS 经 include_str! 引用本文件，
//     落盘到主题目录的 pet.js = 本文件内容 + 注入版宿主壳
//     （壳负责从 variables.css 的 --zbar-pet-* 变量读参数、从
//     window.__ZBAR_USAGE__ 读 usage-data.js 重载数据再喂给核心）；
//   - 独立悬浮窗版：ZBar 自身的透明宠物窗口页面（pet.html）经
//     <script> 加载本文件，宿主壳（pet-main.ts）监听 Tauri 事件
//     喂数据、直接调 setParams。
//
// V2 变更（独立悬浮窗 + 心跳/活动时刻契约）：核心从注入版脚本中抽出为
// 无环境假设的工厂（不再自行读 CSS 变量与 window.__ZBAR_USAGE__，参数与
// 数据一律由宿主喂入）；数据契约新增两个附加字段——
//   hb 心跳时刻：注入版宿主从独立小文件 usage-data-hb.js（每 2 秒重写，
//      仅注入版宠物开启时存在）读到后经 heartbeat(ms) 接口喂入，核心据
//      此判定数据源是否存活（ZBar 退出 → 心跳停滞 → 沉睡）；心跳从未
//      到达时回退用 ts 判陈旧（V1 行为，兼容旧 ZBar/旧数据文件）；
//      独立悬浮窗宿主的事件推流自带新鲜度，同样经 heartbeat 喂入或不喂。
//   la 最后活动时刻：全部完成轮 end 与进行中轮 start 的最大值，闲置
//      判定（idle↔sleeping）改用它——ts 是"数据内容变化时刻"，轮次滑出
//      查询窗口同样会刷新 ts，按 ts 判闲置会让宠物周期性误弹回闲置；
//      la 只随真实活动前进。缺失（旧数据文件）回退用 ts，行为同 V1。
//
// V3 变更（自定义宠物导入，Petdex 格式）：create/setParams 新增可选参数
//   customAsset = { meta, dataUri }——style 形如 "custom:<id>" 且资产有效
//   时启用图集渲染器：加载 dataUri 精灵图集 Image，按 meta 的网格
//   （cols/rows/frameW/frameH）与逐状态行配置（states[状态] = { row,
//   frames, frameMs }）用 drawImage 切帧，imageSmoothingEnabled = false
//   保像素锐利；帧序循环与既有 rAF 动画循环、五状态机、typing 速度
//   分档全部复用。custom 样式缺资产（文件缺失/校验失败）回退内建第一
//   形象；内建 style 路径零改动。资产由宿主注入（注入版壳读同目录
//   pet-custom.js 的 window.__ZBAR_PET_CUSTOM__，独立版宿主 invoke
//   get_custom_pet_asset），核心保持无环境假设。
//
// V4 变更（动画驱动改 setInterval，修复 ZCode 注入环境冻结）：动画循环
//   不再依赖 requestAnimationFrame——ZCode（Electron 多视图架构）的对话
//   页视觉可见但 Chromium 不向其派发 beginFrame，rAF 回调全部暂停，宠物
//   冻结在首帧（同一份核心在独立悬浮窗正常）；定时器路径不受影响（实机
//   证据：注入壳的秒级轮询存活）。改为 setInterval 轮询驱动（ANIM_MS =
//   60ms，约 16fps；宠物最快帧率 typing 快档 95ms/帧，轮询粒度余量充
//   足），帧推进语义（frameAcc 累积 / frameIdx 推进 / drawFrame / 超
//   1000ms 基准重置）与时间基准（performance.now，Date.now 兜底）全部
//   保持不变；页面隐藏的省电交给浏览器对隐藏页定时器的限流自然承担，
//   应用层不再做 document.hidden 判断（核心状态机与注入壳参数轮询的
//   hidden 降频一并移除——visibility 判定与视觉可见性脱节的注入环境
//   下，降频会造成状态切换滞后与心跳断流）。interval 生命周期收敛为
//   build 启动 / teardownDom 清理，顺带修复 setParams 形象热切换重建
//   后动画链不再重启的隐患。独立悬浮窗版同步切换（帧率上限降至轮询
//   粒度，最快档实际约 8fps，观感无差）。
//
// V5 变更（待处理用户消息预判 + working 迟滞，修复两个滞后观感缺陷）：
//   数据契约新增 pu 附加字段（usage-data.js 与独立版事件快照同口径，
//   值 = 最近一条「尚无完成轮」的 user 消息时刻 ms，无则 null/缺失）：
//   - 预判：用户消息发送即落库，但进行中轮通道（runs）只由已完成的
//      模型请求行聚合——首笔请求 30~70 秒（实测主库）才完成落库，期间
//      宠物误停 waiting/sleeping。runs 为空且 pu 距今 < PENDING_TURN_MS
//      时直接输出 working（优先级在 celebrating 之后、idle/sleeping 之前）；
//   - working 迟滞：轮完成落库瞬间 runs 消失、宠物立即回落等待，与下
//      一轮开始前的间隙叠加产生"工作期间反复变回等待"的回落感。离开
//      working/typing（runs 消失且无预判）后距最近一次真实工作态不足
//      WORKING_LINGER_MS 时维持 working 再按原规则回落（lastWorkT 只随
//      真实工作信号前进——runs 活跃或预判命中，迟滞自身不回写，否则
//      45 秒窗口被无限续期）；celebrating 不受迟滞影响，庆祝结束后若
//      在迟滞窗口内回 working，否则回落；
//   - pu 缺失（旧数据文件 / 老版本库）行为不变：预判不生效，迟滞仅由
//      runs 消失触发，向后兼容。状态机判定抽出为模块级纯函数
//      decideState（单测直测），实例侧 computeState 组装闭包侧写。
//
// V6 变更（宠物状态与任务阶段精确匹配，新增 tool_running / failed 两
//   状态）：数据契约新增 ta / fe 两个附加字段（usage-data.js 与独立版
//   事件快照同口径）——
//   - ta（tool active，活跃工具）：ZCode 主库 tool_usage 表的工具调用
//      开始瞬间落库（status='running'）、完成即更新——是「正在执行工具
//      （构建/测试/命令）」的实时信号（比 model_usage 的请求完成落库
//      强得多：工具执行期间模型请求间隙、runs 的 out 停滞）。ta 活跃
//      （now − ta < TOOL_ACTIVE_MS=30s，常量只兜「轮询间隙 + 异常残留」
//      ——正常结束 ta 立即变 null）且 runs 非空或 pu 预判命中时输出
//      tool_running（优先于 typing/working：工具执行期间 out 通常不增
//      长，typing 自然让位）；
//   - fe（failure event，失败轮事件）：最近一次「失败或取消」完成轮的
//      完成时刻（数据侧只在失败轮新增时刷新，成功轮不刷新 fe）。fe
//      距今 < FAILED_MS(3000) 时输出 failed（沮丧 3 秒后回落，优先级
//      最高）；轮完成时按成败分支互斥：成功 → celebrating，失败/取消
//      → failed，不叠加；
//   - 迟滞调整：WORKING_LINGER 迟滞覆盖「工作类状态（tool_running/
//      typing/working）→ 非工作状态」的过渡；工作类状态之间切换不触发
//      迟滞逻辑（lastWorkT 随真实工作信号——含 tool_running——前进，
//      迟滞输出保持 working，pu 预判与 V5 迟滞语义不变）；
//   - 新状态帧映射：自定义形象（Petdex 9/11 行）默认映射 tool_running →
//      行 1 running-right（8 帧，跑腿语义）、failed → 行 5 failed（8 帧），
//      旧五状态 pet.json 缺键由 Rust 侧读取时补默认行 + 本核心
//      customStateDef 对缺失键回退相近状态帧（双保险）；
//   - ta/fe 缺失（旧数据文件 / 老版本库）行为不变：新状态不触发，
//      行为同 V5。
//
// V8 变更（智谱娘转正为默认内置形象，渲染收敛 customAsset-only）：
//   - 删除内建的 cat / bot 两个字符网格形象（PET_STYLES 常量与逐像素
//      绘制路径、调色板、16×16 帧解析一并移除）——全部形象统一走 V3 的
//      Petdex 图集渲染器；
//   - 「智谱 Z 娘」（custom:zhipu-z-niang）改为软件内置形象：资产随
//      安装包分发（Rust 侧 include_bytes 内嵌，启动时释放到 ~/.zbar/
//      pets，见 pets.rs 的 ensure_builtin_pet），默认选中且不可删除，
//      数据通道与用户自定义宠物完全一致（custom: 前缀同管道）；
//   - 渲染语义变化：style 非 custom:* 或 customAsset 未就位/无效时不再
//      回退内建形象，而是进入「空态」——实例保持存活（feed/heartbeat/
//      setParams 照常、状态机照常运转）但不挂任何 DOM（宠物暂隐），
//      宿主在资产就位后经 setParams 热切换重建即恢复显示。旧版壳 +
//      新版 variables.css（或反之）的升级窗口期表现为宠物暂隐而非闪
//      回猫脸，属预期（约 1 秒的资产加载瞬态同理）；
//   - 状态机 decideState 与数据契约（v/ts/la/pu/ta/fe/turns/runs）零
//      改动。
//
// V9 变更（动作语义细分：新增 thinking / walking 两状态）：
//   原-working 一个动画承担了三种语义不同的场景，与任务阶段对不上——
//   「runs 活跃但 out 不增长」是模型思考/规划，「pu 预判命中」是宠物
//   动身去干活，「迟滞保持」是在轮间隙踱步等待。V9 拆分：
//   - thinking：runs 活跃但 out 不增长（新轮刚开始/两步请求之间/模型
//      思考中）——原 working 的第一条触发场景，语义「坐着想」；
//   - walking：pu 预判命中（用户已发消息、runs 尚看不见，宠物「动身
//      去干活」）与迟滞保持（轮完成后的间隙「踱步等待下一轮」）——
//      原 working 的后两条触发场景，语义「走动」；
//   - working 保留为回退状态：decideState 不再输出 working，但形象的
//      states 缺 thinking/walking 键时（老宠物/用户自定义宠物）经
//      CUSTOM_STATE_FALLBACK 回退到 working 帧——此类形象动画行为与
//      V8 完全一致（零回归）；内置智谱娘 pet.json 已随版本补
//      thinking/walking 键（pets.rs 升级机制自动覆盖到用户库）；
//   - 数据契约（v/ts/la/pu/ta/fe/turns/runs）与全部判定常量零改动；
//      迟滞基准 lastWorkT 的推进列表补入 thinking/walking（V6 工作类
//      状态语义的延续——runs 活跃/预判命中时输出什么状态名，迟滞基准
//      就随什么状态前进，迟滞自身仍不回写）。
//
// 对外接口（工厂形态，window.ZBarPet）：
//   var pet = ZBarPet.create(container, {
//     style: "custom:zhipu-z-niang", size: 64,
//     customAsset: { meta, dataUri }
//   });
//   pet.feed({ v: 2, ts, la, pu, ta, fe, turns: [...], runs: [...] });
//   pet.heartbeat(<ms>);          // 可选：喂心跳（陈旧判定改用心跳）
//   pet.setParams({ style: "custom:boba", customAsset }); // 可部分传入
//   pet.destroy();
//   ZBarPet.decideState(now, side);   // 状态机纯判定（单元测试直测用，
//                                      // 正常宿主不消费；side 字段见函数注释）
//
// 状态机（优先级从高到低，详见 decideState）：
//   sleeping    闭眼呼吸（含右侧 Zzz 像素图案）。触发：无有效数据；
//               或心跳 hb 距今 > DATA_FRESH_MS（ZBar 退出/未运行，
//               数据源不再刷新，绝不显示进行中状态）；或 runs 为空、无
//               pu 预判、超出工作迟滞且距最近轮活动 > IDLE_SLEEP_MS。
//   idle        睁眼轻摆/眨眼。触发：runs 为空且无预判/迟滞，
//               IDLE_SLEEP_MS 内有轮活动。
//   thinking    （V9）坐着思考（智谱娘：手托下巴 + 思考气泡）。触发：
//               runs 非空但 out 合计未增长（新轮刚开始/两步请求之间/
//               模型思考规划中）——原 working 的该场景细分而来。
//   walking     （V9）走路踱步。触发：pu 预判命中（用户已发消息、首笔
//               模型请求完成落库前，宠物「动身去干活」）；或工作迟滞
//               保持（轮完成的间隙「踱步等待下一轮」，工作信号刚消失
//               WORKING_LINGER_MS 内；V6 起迟滞覆盖「工作类状态 → 非
//               工作状态」的过渡）——原 working 的两条触发场景细分而来。
//   working     （V9 起 decideState 不再输出）回退状态：形象 states 缺
//               thinking/walking 键时（老宠物/用户自定义宠物）经
//               CUSTOM_STATE_FALLBACK 回退到 working 帧，动画行为与 V8
//               完全一致。
//   typing      奋笔疾书。触发：runs 非空且 out 合计在增长；动画速度按
//               token 增速（两次数据刷新间 out 差值 / 间隔秒数）分 3 档。
//   tool_running（V6）替主人跑腿执行。触发：ta 活跃（工具 running 行
//               开始落库，now − ta < TOOL_ACTIVE_MS）且 runs 非空或 pu
//               预判命中；优先于 typing/thinking（工具执行期间模型请求
//               间隙，out 通常不增长）。自定义形象走 Petdex 行 1
//               running-right。
//   celebrating 跳跃庆祝约 CELEBRATE_MS 后回落。触发：turns 数组
//               新增且最近完成轮非失败（成败互斥，见 failed）。
//   failed      （V6）沮丧垂头 3 秒（FAILED_MS）后回落。触发：fe 距今
//               < FAILED_MS（最近完成轮失败/取消，数据侧 fe 只在失败轮
//               新增时刷新）；优先级最高（心跳陈旧短路除外）。自定义形
//               象走 Petdex 行 5 failed。
// ============================================================
(function () {
  "use strict";

  /* ---- 常量（集中此处，便于实机比对调整） ---- */
  var ATTR_ROOT = "data-zbar-pet"; /* 宠物容器标记（防重复挂载与清理定位） */
  var TICK_MS = 1000; /* 状态机轮询周期（宿主不必驱动，核心自治） */
  var ANIM_MS = 60; /* 动画轮询周期（V4：setInterval 驱动，约 16fps；
    宠物最快帧率 typing 快档 95ms/帧，60ms 粒度余量充足；不用 rAF——
    ZCode 注入环境视觉可见但无 beginFrame，rAF 全暂停 → 冻结首帧） */
  var DATA_FRESH_MS = 10000; /* 心跳 hb 距今超此值视为陈旧 → 强制沉睡 */
  var IDLE_SLEEP_MS = 60000; /* runs 为空且距最近轮活动超此值 → 入睡 */
  var CELEBRATE_MS = 3000; /* 轮完成庆祝动画时长 */
  var PENDING_TURN_MS = 90000; /* V5 待处理用户消息预判窗口：用户发消息
    （message 表发送即落库）后首笔模型请求完成落库前 runs 通道看不见，
    实测主库该窗口 30~70 秒，取 90 秒留余量；超窗即放弃预判（消息异常
    中断、永不落轮等边界由此时限兜底，不放大陈旧信号） */
  var WORKING_LINGER_MS = 45000; /* V5 working 迟滞窗口：轮完成落库后
    runs 消失且无 pu 预判时，距最近一次真实工作态不足此值则维持
    working 再回落——抹平"轮完成 → 回落等待 → 下一轮开始"间隙的回落
    感（轮间隙典型几十秒内）；lastWorkT 只随真实工作信号前进，迟滞
    自身不续期，窗口不会被无限延长。V6 起迟滞语义覆盖「工作类状态
    （tool_running/typing/working）→ 非工作状态」的过渡，工作类状态
    之间切换不触发迟滞逻辑 */
  var TOOL_ACTIVE_MS = 30000; /* V6 工具活跃窗口：ta（工具 running 行的
    started_at）距今小于此值视为工具在执行。工具 running 行正常完成时
    数据侧立即更新为 completed（ta 归 null），常量只兜「轮询间隙 +
    异常残留」（数据侧另有 10 分钟窗口剔除崩溃残留的 running 行，此处
    30 秒为第二层兜底：轮询 2 秒 + 数据龄 + 时钟偏差余量） */
  var FAILED_MS = 3000; /* V6 失败沮丧时长（与 CELEBRATE_MS 同款常量口径）：
    fe（最近一次失败/取消轮的完成时刻）距今小于此值显示 failed，
    超时自然回落（消费端按时间戳判窗，无需显式清理） */
  var SPEED_TIERS = [15, 60]; /* typing 速度分档阈值（tok/s：≥15 中档、≥60 快档） */
  /* 状态全集（V6 新增 tool_running/failed，V9 新增 thinking/walking）：
   * 均由 meta.states 的行配置渲染（旧五状态 pet.json 缺 tool_running/
   * failed 键时由 Rust 侧读取补默认行 + 本核心 customStateDef 回退双
   * 保险；thinking/walking 键 Rust 侧不补默认行——通用 v2 宠物作者自
   * 命名的行语义各不相同，细分映射只随内置 pet.json 分发，缺键一律经
   * 下方回退表落到 working 帧，老宠物/用户自定义宠物行为与 V8 一致） */
  var STATES = [
    "sleeping",
    "idle",
    "working",
    "typing",
    "celebrating",
    "tool_running",
    "failed",
    "thinking",
    "walking"
  ];
  /* 自定义形象缺键回退（双保险：Rust 侧读取旧五状态 pet.json 时已补
   * tool_running/failed 默认行，此处兜底未补齐的路径——meta.states 缺
   * 新状态键时回退到语义相近的既有键，不落到 {row:0,frames:1} 的兜底
   * 帧。V9 追加：thinking/walking 缺键（未随细分映射升级的形象）回退
   * working——原 working 的三条触发场景即这两状态的来源，观感与 V8
   * 完全一致） */
  var CUSTOM_STATE_FALLBACK = {
    tool_running: "typing",
    failed: "sleeping",
    thinking: "working",
    walking: "working"
  };
  var CUSTOM_PREFIX = "custom:"; /* 形象 style 前缀（V8 起全部形象的唯一
    形态：后接宠物 id，内置智谱娘 = custom:zhipu-z-niang） */

  /* 单调时钟（与 rAF 时间戳同源的 performance.now）：性能计时器缺失
   * 的极端环境兜底 Date.now（动画拍基准用，可接受） */
  function nowMs() {
    return typeof performance !== "undefined" && performance && performance.now
      ? performance.now()
      : Date.now();
  }

  /* ---- 形象资产（V3 引入，V8 起唯一渲染通道）：style 形如 "custom:<id>"
   *      且宿主注入了有效资产（{ meta, dataUri }）时走图集渲染器。缺资产
   *      一律进入空态（不渲染，见 effectiveStyleOf——宿主侧文件缺失/
   *      校验失败的静默降级语义，资产就位后经 setParams 热切换恢复） ---- */
  function customAssetValid(asset) {
    return !!(
      asset &&
      typeof asset.dataUri === "string" &&
      asset.dataUri &&
      asset.meta &&
      typeof asset.meta === "object" &&
      isFinite(asset.meta.frameW) &&
      asset.meta.frameW > 0 &&
      isFinite(asset.meta.frameH) &&
      asset.meta.frameH > 0 &&
      asset.meta.states
    );
  }

  /* 形象归一（V8 语义）：custom 前缀且资产有效 → 原值直通（渲染）；
   * 其余（非 custom 前缀 / 资产缺失或无效）→ 空串（空态，不渲染）。
   * 不再有内建回退值——升级窗口期（新版壳 + 旧 variables.css 的
   * cat/bot 残留值等）表现为宠物暂隐而非闪错形象 */
  function effectiveStyleOf(raw, asset) {
    if (typeof raw === "string" && raw.indexOf(CUSTOM_PREFIX) === 0) {
      return customAssetValid(asset) ? raw : "";
    }
    return "";
  }

  /* ---- 状态机（V5 拆为模块级纯函数 decideState + 实例侧组装）：
   *      输入数据快照侧写 + 当前时刻，输出状态名；陈旧判定用心跳
   *      （heartbeat 接口喂入，ZBar 退出 → 心跳停滞 → 绝不显示进行中
   *      状态；心跳从未到达回退 ts），闲置判定用最后活动时刻 la（轮次
   *      滑出查询窗口不刷新 la，防周期性误弹闲置），V5 新增 pu 预判与
   *      working 迟滞，V6 新增 failed（失败沮丧）/tool_running（工具
   *      执行）两状态，V9 将原 working 的三条触发场景细分为 thinking
   *      （runs 活跃但 out 不增长）与 walking（pu 预判/迟滞保持）——
   *      working 降为缺键形象的回退帧目标。side 字段（纯数据，单测直
   *      测）：{ hasData, lastHb, runsActive, outGrowing, celebrateUntil,
   *      failedUntil, toolActive, pending, lastWorkT, lastActivity }，其中
   *      pending/toolActive 为调用方按 pu/PENDING_TURN_MS 与 ta/
   *      TOOL_ACTIVE_MS 预计算的布尔、failedUntil 为按 fe + FAILED_MS
   *      预计算的沮丧截止时刻 ---- */
  function decideState(now, s) {
    if (!s.hasData) return "sleeping";
    /* 心跳陈旧：数据源不在（ZBar 退出/未运行），绝不显示进行中状态
     * （含 failed 沮丧——fe 数据随数据源一起陈旧） */
    if (now - s.lastHb > DATA_FRESH_MS) return "sleeping";
    /* V6 失败沮丧：最近完成轮失败/取消（fe 刚刷新，failedUntil 在
     * 窗口内）→ 优先级最高的短暂高亮；与 celebrating 互斥（feed 的
     * 成败分支保证不同时置位，此处顺序兜底） */
    if (s.failedUntil > now) return "failed";
    if (s.runsActive) {
      /* V6 工具执行：工具 running 行活跃（ta 在窗口内）时优先于
       * typing/thinking——工具执行期间模型请求间隙，out 通常不增长，
       * typing 自然让位（「替主人跑腿执行」观感） */
      if (s.toolActive) return "tool_running";
      /* V9 细分：runs 活跃但 out 不增长 = 模型正在思考/规划（新轮
       * 刚开始、两步请求之间、长推理中）→ thinking（V8 为 working） */
      return s.outGrowing ? "typing" : "thinking";
    }
    if (s.celebrateUntil > now) return "celebrating";
    /* V5 预判：runs 为空但最近有待处理用户消息（发送即落库、首笔模型
     * 请求完成落库前 runs 通道看不见，实测窗口 30~70 秒）→ 先行进入
     * 动身观感，消除用户发消息后的滞后；优先级在 celebrating 之后。
     * V6：预判期间工具已开始跑（ZCode 收到消息即执行首个工具，首笔
     * 模型请求可能仍未落库）→ tool_running 更准确。V9：预判输出
     * walking（「动身去干活」，V8 为 working） */
    if (s.pending) {
      if (s.toolActive) return "tool_running";
      return "walking";
    }
    /* V5 迟滞：工作信号刚消失（轮完成落库 → runs 消失且无预判）时，
     * 距最近一次真实工作态不足 WORKING_LINGER_MS 则维持动身观感，抹平
     * 轮间隙的回落感。V6 起迟滞语义为「工作类状态（tool_running/
     * typing/thinking/walking）→ 非工作状态」的过渡——迟滞输出统一
     * walking（V9，「踱步等待下一轮」；V8 为 working），工作类状态之间
     * 的切换在上面的分支内完成、不走迟滞；celebrating 分支在前不受
     * 迟滞影响（庆祝结束后落回此处，在窗口内回 walking，否则按原规则
     * 回落） */
    if (s.lastWorkT > 0 && now - s.lastWorkT < WORKING_LINGER_MS) {
      return "walking";
    }
    return now - s.lastActivity < IDLE_SLEEP_MS ? "idle" : "sleeping";
  }

  /* ---- 渲染：DOM 容器 + canvas 图集切帧（V8 起 customAsset 唯一路径）。
   *      画布逻辑尺寸 = 帧尺寸（meta.frameW × frameH），CSS 尺寸由 size
   *      参数内联驱动 + image-rendering:pixelated 最近邻放大（像素风关
   *      键），改大小零重建成本 ---- */
  /**
   * 创建宠物实例并挂载到 container。
   * @param container 挂载点元素（宠物的定位方式由宿主环境负责：
   *                  注入版宿主给容器配 fixed 定位样式，独立窗口版
   *                  挂进占满窗口的普通 div）
   * @param opts { style: 形象值（custom:<id>，V8 起唯一合法形态）,
   *              size: 显示边长 px,
   *              customAsset: 形象资产 { meta, dataUri }（V8 起渲染必需，
   *              缺失时进入空态——实例存活但不挂 DOM，等待宿主在资产
   *              就位后 setParams 热切换重建） }
   * @returns 实例：{ feed(data), setParams({style,size,customAsset}),
   *          heartbeat(ms), destroy() }；container 不可用时返回 null
   */
  function create(container, opts) {
    if (!container || !container.appendChild) return null;
    opts = opts || {};

    var root = null;
    var canvas = null;
    var ctx = null;
    var curStyleId = ""; /* 当前生效形象（空串 = 空态未渲染，V8 语义） */
    var curSize = 0;
    /* ---- 形象资产状态（V3 引入）：custom = 当前资产引用；customImg 为已
     *      解码图集 Image；customRatio = frameH/frameW（CSS 高按宽等比
     *      缩放，保持 Petdex 帧 192×208 的非正方形比例不拉伸） ---- */
    var custom = null;
    var customImg = null;
    var customImgOk = false;
    var customRatio = 1;
    var state = "sleeping";
    var frameIdx = 0;
    var frameAcc = 0;
    var lastFrameT = 0;
    var animTimer = 0; /* 动画轮询 interval（V4：build 启动 / teardownDom 清理） */
    var tickTimer = 0;
    var destroyed = false;

    /* ---- 数据快照侧写（feed 时更新，computeState 消费） ---- */
    var hasData = false;
    var dataTs = 0; /* 数据时间戳（最后数据变化时刻） */
    var lastHb = 0; /* 最近心跳时刻（heartbeat 喂入；从未喂过回退 ts） */
    var hbSeen = false; /* 是否收到过有效心跳（决定陈旧判定回退口径） */
    var lastActivity = 0; /* 最后活动时刻（la 字段；缺失回退 ts，闲置判定消费） */
    var pendingUser = 0; /* 待处理用户消息时刻（pu 字段，V5；0 = 无信号：
      pu 缺失（旧数据文件）与 null 同样归 0，预判不生效，行为同 V4） */
    var toolActiveAt = 0; /* 活跃工具时刻（ta 字段，V6；0 = 无信号：ta
      缺失（旧数据文件）与 null 同样归 0，tool_running 不触发，行为同
      V5） */
    var failedUntil = 0; /* 沮丧截止时刻（V6；0 = 无沮丧。轮完成且 fe 在
      近 FAILED_MS*2 内刷新时置位（celebrateUntil 同款「feed 置位 +
      decideState 判窗」模式）——数据侧 fe 只在失败轮新增时变化，成功
      轮完成不刷新 fe，据此与 celebrating 互斥不叠加 */
    var lastWorkT = 0; /* 最近一次真实工作态时刻（V5 迟滞基准；只随
      runs 活跃或预判命中的 working/typing 前进，迟滞自身不回写） */
    var runsActive = false; /* 快照 runs 是否非空 */
    var outGrowing = false; /* 两次数据刷新间 out 合计是否增长 */
    var speedTier = 0; /* typing 速度档（0 慢 / 1 中 / 2 快） */
    var celebrateUntil = 0; /* 庆祝截止时刻（0 = 无庆祝） */
    var prevTurnsCount = -1; /* 上次快照 turns 数量（-1 = 尚无基准） */
    var prevLastKey = ""; /* 上次快照末尾轮标识（turn|umid） */
    var prevOutTotal = 0; /* 上次快照 runs out 合计（含并入的 sub.out） */
    var prevDataAt = 0; /* 上次数据到达的本地时刻（算增速用） */

    /* 自定义形象的逐状态行配置（缺项兜底第 0 行单帧，坏配置不抛错）。
     * V6：缺新状态键（tool_running/failed——旧五状态 pet.json 未补齐
     * 的路径）时先回退到相近状态的既有键（双保险，与 Rust 侧读取时补
     * 默认行配合），再落到第 0 行兜底。V9：thinking/walking 键只随内置
     * pet.json 分发（Rust 侧不补默认行），缺键形象回退 working 帧 */
    function customStateDef(name) {
      var states = custom.meta.states || {};
      var def = states[name];
      if (!def && CUSTOM_STATE_FALLBACK[name]) def = states[CUSTOM_STATE_FALLBACK[name]];
      if (!def) return { row: 0, frames: 1, frameMs: 400 };
      return {
        row: Math.max(0, parseInt(def.row, 10) || 0),
        frames: Math.max(1, parseInt(def.frames, 10) || 1),
        frameMs: def.frameMs
      };
    }

    function drawFrame() {
      if (!ctx) return;
      /* ---- 图集切帧 drawImage（V8 起唯一渲染路径；像素锐利关键：
       *      imageSmoothingEnabled = false 最近邻采样）；图集未就绪
       *      （异步 decode 中/加载失败）保持空白帧，就绪后 onload 触发
       *      重绘 ---- */
      if (!custom) return;
      if (!customImgOk || !customImg) return;
      var def = customStateDef(state);
      var fw = custom.meta.frameW;
      var fh = custom.meta.frameH;
      var col = frameIdx % def.frames;
      ctx.clearRect(0, 0, fw, fh);
      ctx.imageSmoothingEnabled = false;
      try {
        ctx.drawImage(
          customImg,
          col * fw,
          def.row * fh,
          fw,
          fh,
          0,
          0,
          fw,
          fh
        );
      } catch (e) {
        /* 坏图（截断/尺寸不符）静默保持空白 */
      }
    }

    function setState(next) {
      if (next === state) return;
      state = next;
      frameIdx = 0;
      frameAcc = 0;
      drawFrame();
    }

    /* 当前状态的每帧停留毫秒（typing 按速度档选；读 meta.states 的
     * frameMs，同样支持 typing 数组分档） */
    function frameMsOf() {
      var fmC = customStateDef(state).frameMs;
      if (state === "typing" && Array.isArray(fmC)) {
        return fmC[Math.min(speedTier, fmC.length - 1)] || 150;
      }
      return typeof fmC === "number" && fmC > 0 ? fmC : 400;
    }

    /* 动画轮询循环（V4：setInterval 驱动，不再依赖 rAF——ZCode 注入
     * 环境的对话页视觉可见但 Chromium 不向其派发 beginFrame，rAF 回调
     * 全部暂停导致宠物冻结首帧；定时器路径不受影响）。帧推进语义与
     * rAF 版完全一致；页面隐藏的省电交给浏览器对定时器的限流自然
     * 承担，应用层不做 visibility 判断 */
    function loop() {
      if (destroyed) return;
      try {
        if (!root) return; /* 空态（V8）：未挂 DOM 时无帧可推 */
        var t = nowMs();
        /* 自定义形象：帧数来自 meta.states（图集就绪前空转等待） */
        if (customImgOk) {
          var fmsC = frameMsOf();
          if (lastFrameT === 0 || t - lastFrameT > 1000) lastFrameT = t;
          frameAcc += t - lastFrameT;
          lastFrameT = t;
          if (frameAcc >= fmsC) {
            frameIdx = (frameIdx + 1) % customStateDef(state).frames;
            frameAcc = 0;
            drawFrame();
          }
        } else {
          lastFrameT = t; /* 未就绪：不吃时间债，就绪后从当前拍起 */
        }
      } catch (e) {
        /* 静默 */
      }
    }

    /* ---- 数据消费：宿主把 usage-data.js 同构数据喂进来（注入版宿主读
     *      window.__ZBAR_USAGE__，独立窗口宿主监听 Tauri 事件）。ts 未变
     *      说明无新数据（数据源内容无变化跳写，ts 停留在最后数据变化
     *      时刻，语义即"最近一次轮活动"）→ 跳过重算。la（最后活动时刻）
     *      与 ts 同批更新：闲置判定改用 la，轮次滑出宿主的查询窗口时
     *      ts 会刷新而 la 不会（见文件头 V2 说明），宠物不再误弹闲置。
     *      心跳独立于数据：注入版宿主从 usage-data-hb.js 读到后经
     *      heartbeat(ms) 喂入；心跳从未到达时此处回退 ts 判陈旧。
     *      快照对比：turns 数量 + 末尾轮标识（庆祝触发）、runs 各行
     *      out/req 聚合（工作判定与速度分档） ---- */
    function feed(d) {
      if (destroyed || !d || d.v !== 2) return;
      if (!hbSeen) lastHb = d.ts; /* 心跳缺失回退 ts（V1 陈旧判定口径） */
      if (d.ts === dataTs) return; /* 无新数据，跳过重算 */

      var now = Date.now();
      var dtSec = prevDataAt > 0 ? (now - prevDataAt) / 1000 : 2;

      /* runs 聚合：活动判定 + out 合计（子代理行数值已并入主轮行 sub，
       * 跳过打 m 标记的行防双计，口径与 usage.js 会话累计一致） */
      var runs = d.runs || [];
      var active = false;
      var outTotal = 0;
      for (var i = 0; i < runs.length; i++) {
        var r = runs[i];
        if (!r || r.m) continue;
        active = true;
        outTotal += (r.out || 0) + (r.sub ? r.sub.out || 0 : 0);
      }

      /* turns 对比：数量增加或末尾标识变化 → 轮完成；V6 成败互斥分支：
       * fe 在近 FAILED_MS*2 内刷新 = 新完成轮为失败/取消轮（数据侧 fe
       * 只在失败轮新增时变化，成功轮不刷新 fe；余量 2 倍覆盖「完成落库
       * → 轮询 → 重载 → 消费」的数据龄，典型 0~4 秒）→ 沮丧 3 秒且不
       * 庆祝；否则照旧庆祝。极端窄边缘：6 秒内先失败一轮又成功完成一
       * 轮时成功轮不庆祝——最近体验里有失败，显示沮丧亦合理 */
      var turns = d.turns || [];
      var count = turns.length;
      var last = turns[count - 1];
      var lastKey = last ? (last.turn || "") + "|" + (last.umid || "") : "";
      var feFresh = typeof d.fe === "number" && d.fe > 0;
      if (
        prevTurnsCount >= 0 &&
        lastKey !== "" &&
        (count > prevTurnsCount ||
          (count === prevTurnsCount && lastKey !== prevLastKey))
      ) {
        if (feFresh && now - d.fe < FAILED_MS * 2) {
          failedUntil = now + FAILED_MS;
          celebrateUntil = 0;
        } else {
          celebrateUntil = now + CELEBRATE_MS;
        }
      }

      if (active) {
        /* out 增长与速度分档（typing 观感随 token 增速分 3 档） */
        var speed = dtSec > 0 ? Math.max(0, (outTotal - prevOutTotal) / dtSec) : 0;
        outGrowing = outTotal > prevOutTotal;
        speedTier = speed >= SPEED_TIERS[1] ? 2 : speed >= SPEED_TIERS[0] ? 1 : 0;
        /* 新轮开始（runs 从无到有）打断庆祝：立即进入工作观感 */
        if (!runsActive) celebrateUntil = 0;
      } else {
        outGrowing = false;
        speedTier = 0;
      }

      runsActive = active;
      prevTurnsCount = count;
      prevLastKey = lastKey;
      prevOutTotal = outTotal;
      dataTs = d.ts;
      /* la 缺失（旧数据文件）回退 ts：闲置判定口径与 V1 一致 */
      lastActivity = typeof d.la === "number" ? d.la : d.ts;
      /* pu（V5）：待处理用户消息时刻；数据侧契约保证 pu 变化必伴随 ts
       * 变化（参与内容对比——用户发消息本身就是数据变化），此处随新
       * 数据一并更新即可。缺失（旧数据文件）与 null 归 0，预判不生效 */
      pendingUser = typeof d.pu === "number" && d.pu > 0 ? d.pu : 0;
      /* ta（V6）：活跃工具时刻，同 pu 契约（工具开始/结束都参与内容
       * 对比、变化必伴随 ts 变化）。缺失与 null 归 0，tool_running
       * 不触发，行为同 V5 */
      toolActiveAt = typeof d.ta === "number" && d.ta > 0 ? d.ta : 0;
      prevDataAt = now;
      hasData = true;
    }

    /* ---- 心跳喂入（独立于数据）：注入版宿主从 usage-data-hb.js 读到
     *      后每 2 秒调一次；收到过有效心跳后陈旧判定不再回退 ts
     *      （长思考期间 ts 停滞但心跳鲜活，宠物保持工作/思考观感） ---- */
    function heartbeat(ms) {
      if (destroyed) return;
      if (typeof ms === "number" && ms > 0) {
        lastHb = ms;
        hbSeen = true;
      }
    }

    /* 实例侧组装：闭包侧写喂给模块级纯判定 decideState，并维护迟滞基准
     * lastWorkT（只随真实工作信号前进——runs 活跃或预判命中；迟滞自身
     * 的 walking 不回写，否则 45 秒窗口会被无限续期。V6：tool_running
     * 也是真实工作信号——迟滞语义覆盖「工作类状态 → 非工作状态」的
     * 过渡，从任一工作类状态回落时均由迟滞窗口维持动身观感。V9：
     * thinking/walking 是 working 三场景的细分输出，同样随真实工作信号
     * 推进迟滞基准；working 保留在列表中作缺键回退形态的防御） */
    function computeState(now) {
      var pending =
        pendingUser > 0 && now - pendingUser < PENDING_TURN_MS;
      var toolActive =
        toolActiveAt > 0 && now - toolActiveAt < TOOL_ACTIVE_MS;
      var st = decideState(now, {
        hasData: hasData,
        lastHb: lastHb,
        runsActive: runsActive,
        outGrowing: outGrowing,
        celebrateUntil: celebrateUntil,
        failedUntil: failedUntil,
        toolActive: toolActive,
        pending: pending,
        lastWorkT: lastWorkT,
        lastActivity: lastActivity
      });
      if (
        (runsActive || pending) &&
        (st === "working" ||
          st === "typing" ||
          st === "tool_running" ||
          st === "thinking" ||
          st === "walking")
      ) {
        lastWorkT = now;
      }
      return st;
    }

    /* 状态机轮询：核心自治（宿主只管喂参数与数据）。不做页面隐藏降频
     * （V4）：注入环境的 visibility 判定与视觉可见性脱节，应用层降频会
     * 造成状态切换滞后；隐藏时的省电交给浏览器定时器限流。空态（V8，
     * 资产未就位）下照常运转——状态照判，仅无渲染 */
    function tick() {
      if (destroyed) return;
      try {
        setState(computeState(Date.now()));
      } catch (e) {
        /* 静默 */
      }
      tickTimer = setTimeout(tick, TICK_MS);
    }

    function applySize(size) {
      var n = parseInt(size, 10);
      if (!isFinite(n) || n <= 0 || n === curSize || !canvas) return;
      curSize = n;
      canvas.style.width = n + "px";
      /* 图集帧常为 192×208 非正方形：CSS 高按宽等比缩放（Petdex
       * 桌面端 aspect-ratio 192/208 同款口径），不拉伸变形 */
      canvas.style.height =
        Math.round(n * customRatio * 100) / 100 + "px";
    }

    function build(styleId, size, asset) {
      var wantCustom =
        typeof styleId === "string" &&
        styleId.indexOf(CUSTOM_PREFIX) === 0 &&
        customAssetValid(asset);
      if (!wantCustom) {
        /* ---- V8 空态：style 非 custom:* 或资产未就位/无效 → 不渲染。
         *    清掉一切残留 DOM 与资产状态，实例保持存活（返回 true，
         *    tick/feed/heartbeat 照常运转），curStyleId 置空串——宿主
         *    在资产就位后 setParams（want 从 "" 变为 custom:<id>）
         *    即触发重建热切换恢复显示 ---- */
        teardownDom();
        var emptySize = parseInt(size, 10);
        curSize = isFinite(emptySize) && emptySize > 0 ? emptySize : 0;
        return true;
      }
      /* 防重复挂载：清理任何残留的旧容器（正常不发生，防御性） */
      var old = container.querySelector
        ? container.querySelector("[" + ATTR_ROOT + "]")
        : null;
      if (old && old.parentNode) old.parentNode.removeChild(old);
      root = document.createElement("div");
      root.setAttribute(ATTR_ROOT, "");
      root.style.display = "block";
      canvas = document.createElement("canvas");
      /* ---- 图集形象：画布逻辑尺寸 = 帧尺寸，加载图集后按网格切帧 ---- */
      var meta = asset.meta;
      var fw = Math.max(1, parseInt(meta.frameW, 10) || 1);
      var fh = Math.max(1, parseInt(meta.frameH, 10) || 1);
      canvas.width = fw;
      canvas.height = fh;
      custom = asset;
      customRatio = fh / fw;
      customImgOk = false;
      customImg = new Image();
      customImg.onload = function () {
        if (destroyed || custom !== asset) return;
        customImgOk = true;
        drawFrame();
      };
      customImg.onerror = function () {
        customImgOk = false; /* 坏图静默保持空白 */
      };
      customImg.src = asset.dataUri;
      /* 像素风关键：canvas CSS 尺寸由 size 内联放大 +
       * image-rendering:pixelated 最近邻采样 */
      canvas.style.cssText = "display:block;image-rendering:pixelated;";
      root.appendChild(canvas);
      container.appendChild(root);
      ctx = canvas.getContext ? canvas.getContext("2d") : null;
      if (!ctx) {
        teardownDom();
        return false;
      }
      curStyleId = styleId;
      state = "sleeping";
      frameIdx = 0;
      frameAcc = 0;
      lastFrameT = 0;
      curSize = 0; /* 强制 applySize 生效 */
      applySize(size || 64);
      drawFrame();
      /* 动画轮询随 build 启动（V4）：setParams 形象热切换走 teardownDom
       * （清理）→ build（重启），重建后动画链不再断裂；防重复挂 interval */
      if (animTimer) clearInterval(animTimer);
      animTimer = setInterval(loop, ANIM_MS);
      return true;
    }

    function teardownDom() {
      if (animTimer) {
        clearInterval(animTimer);
        animTimer = 0;
      }
      if (root && root.parentNode) root.parentNode.removeChild(root);
      root = null;
      canvas = null;
      ctx = null;
      curStyleId = "";
      custom = null;
      customImg = null;
      customImgOk = false;
      customRatio = 1;
    }

    /* ---- 对外接口 ---- */
    var instance = {
      /* 喂入 usage-data.js 同构数据（见文件头接口注释） */
      feed: feed,
      /* 喂心跳（可选，注入版宿主每 2 秒调；陈旧判定据此切换口径） */
      heartbeat: heartbeat,
      /* 更新参数（可部分传入：{style} / {size} / {customAsset}），形象
       * 热切换重建画布。custom 样式必须配 customAsset（宿主在选中/
       * 重导入后传入新资产对象）；同一 custom id 的资产对象变化（重导
       * 入替换）同样触发重建，保证帧数据即时刷新；资产从无到有（空态
       * → 渲染）经 want 变化同样触发重建 */
      setParams: function (params) {
        if (destroyed || !params) return;
        var raw =
          params.style !== undefined ? params.style : curStyleId;
        var asset =
          params.customAsset !== undefined ? params.customAsset : custom;
        var want = effectiveStyleOf(raw, asset);
        var assetChanged =
          custom !== null && asset !== custom && want === curStyleId;
        if (want !== curStyleId || assetChanged || !root) {
          teardownDom();
          if (
            !build(
              want,
              params.size !== undefined ? params.size : curSize,
              want.indexOf(CUSTOM_PREFIX) === 0 ? asset : null
            )
          ) {
            return;
          }
          return;
        }
        if (params.size !== undefined) applySize(params.size);
      },
      /* 销毁实例：移除宠物 DOM、停动画与状态机轮询（不可复用） */
      destroy: function () {
        if (destroyed) return;
        destroyed = true;
        if (tickTimer) {
          clearTimeout(tickTimer);
          tickTimer = 0;
        }
        teardownDom();
      }
    };

    if (!build(effectiveStyleOf(opts.style, opts.customAsset), opts.size, opts.customAsset)) {
      return null;
    }
    tick();
    return instance;
  }

  /* 全局工厂：两宿主共用（pet.js 与 pet.html 各自加载一次）。decideState
   * 为状态机纯判定的直测入口（单元测试消费，正常宿主不使用） */
  window.ZBarPet = { create: create };
  window.ZBarPet.decideState = decideState;
})();

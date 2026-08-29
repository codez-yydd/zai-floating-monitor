//! asar 注入物：主题模板（全部自研）、注入块幂等插入与 variables.css 生成。
//!
//! 注入方式：在 ZCode 的 asar 内 out/renderer/index.html 中，
//! `</head>` 前插入两个外链样式（variables.css + theme.css）、
//! `</body>` 前插入 defer 脚本（effects.js + usage.js），全部指向
//! ~/.zbar/agent-themes/ 下的主题文件（file:// URL，不改动应用其它资源）。
//! 注入的外链带 data-zbar-variables / data-zbar-theme / data-zbar-effects /
//! data-zbar-usage 标记：effects.js 热重载靠 data-zbar-variables 定位
//! variables.css（旧版注入行无 data 属性时回退按 href 匹配，同样可热重载）；
//! usage.js 靠自身 src 推导同目录 usage-data.js 的地址。
//! 整段引用包裹在 <!--ZBAR-THEME-BEGIN--> … <!--ZBAR-THEME-END--> 标记内，
//! 重复安装时先剥离旧标记块再插入新块，保证幂等。

use crate::agent_theme::store::ThemeParams;
use std::fs;
use std::path::{Path, PathBuf};

/// 注入块起止标记（head 与 body 两处各一个块，剥离时全部移除）
pub const INJECT_BEGIN: &str = "<!--ZBAR-THEME-BEGIN-->";
pub const INJECT_END: &str = "<!--ZBAR-THEME-END-->";

// ============================================================
// 内置模板
// ============================================================

/// 主题样式模板：消费 variables.css 中的 --zbar-* 变量。
/// 版本化落盘（头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）：
/// 旧版本文件由 ensure_theme_assets 自动覆盖升级；已是当前版本的文件
/// （可能被实机调优过）不会被覆盖。
pub const THEME_CSS: &str = r#"/* ============================================================
 * ZBAR-THEME-V11
 * ZBar Agent 动态壁纸主题样式（由 ZBar 落盘并随版本升级覆盖）
 * ============================================================
 * V11 变更（透出 Windows 端全视口应用壳容器）：Windows 实机
 * （ZCode 3.10.1.6272 / Electron 41）确认应用壳的全视口 flex 容器
 * （div.flex.h-dvh.flex-col.overflow-hidden）自带不透明底色
 * rgb(43 43 43)（浅色主题为对应浅色值），处于普通文档流层，会把
 * 全部负 z-index 壁纸层（视频 -2 / 压暗遮罩 -1）整体盖住——这是
 * Windows 端"不透明度等参数生效但壁纸永不显示"的直接原因。规则
 * 见文件末尾"V11：透出全视口应用壳容器"；macOS 端该容器本就
 * 透明（或不存在），规则无副作用。
 *
 * 实机结论（解剖 ZCode 自身样式）：界面容器普遍通过工具类
 * （.bg-background / .bg-panel 等）消费设计 token
 * --color-background / --color-background-alt / --color-panel，
 * body 无全局背景。因此本文件不再猜测 DOM 选择器，而是直接把
 * 这些 token 重定义为"原色 × 透明度"的 color-mix 半透明色，
 * 所有消费方一次性透出底下的壁纸视频。
 *
 * V3 变更：侧栏改由专用 token --color-sidebar 承载（ZCode 的
 * .bg-sidebar 工具类引用）。
 *
 * V4 变更：实机进一步确认，主侧栏容器与主内容列自身无背景工具
 * 类（透明，透出全局底色），并不消费上述 token，token 覆盖对
 * 它们无效。故追加元素选择器规则（!important）为这两块主区域
 * 直接刷半透明背景。
 *
 * V5 变更（主题分层重构）：V4 及此前全局 token 由滑块变量驱动，
 * 拖任一滑块会牵连顶栏、右侧面板、卡片等一切全局底消费方；且
 * 侧栏底下透出的也是全局底，两个滑块必然联动。V5 起全部 8 条
 * 全局底色 token（:root 与 .dark 的 background / background-alt /
 * panel / sidebar）改由固定氛围变量 --zbar-base-alpha（variables.css
 * 恒定输出 0.25）驱动，与滑块彻底解绑。
 *
 * V6 变更（对话区作用区修正 + 右栏独立滑块）：实测对话列与右侧
 * 面板同属一个整体主内容容器，V5 把对话区滑块刷在该容器上会牵连
 * 右侧面板。V6 删除该容器的元素规则，改按 ZCode 内
 * react-resizable-panels 的面板属性定位（面板无原生 DOM id，属性名
 * data-panel-id，对话列属性值为 conversation-column）：对话列唯一
 * 作用区为按该属性值反查的元素规则（消费对话区滑块变量）；新增
 * 右栏独立滑块变量 --zbar-sidebar-right-opacity，作用于除对话列外
 * 的全部面板（面板属性 :not 反选，自动覆盖将来新增的面板）。
 * 自此三区域分层：左栏侧栏容器 / 对话列 / 右栏其余面板，三个滑块
 * 各管各的容器、互不牵连。
 *
 * V7 变更（选择器属性名修正）：实机解剖面板组件渲染代码确认，
 * 面板容器 DOM 属性名为 data-pane-id（V6 误写为 data-panel-id，
 * 致对话列与右栏两处选择器全部落空：右栏滑块无效果、对话滑块
 * 作用区丢失）。V7 仅把四处选择器的属性名改为 data-pane-id，
 * 规则结构、滑块变量与三区域分层不变。
 *
 * V8 变更（选择器归属修正）：实机解剖确认对话列面板（面板属性
 * 值 conversation-column）内部还嵌套一层子面板组，子面板同样带
 * 面板属性（对话内容 conversation、终端 terminal）。V7 的右栏
 * :not 反选只排除了外层对话列，把这两个子面板也捞进右栏作用区
 * （右栏滑块实际控制了对话区）；而对话区规则只刷外层面板，内层
 * 子面板的背景盖在其上，看似无效果。V8 把对话区作用区扩为三面
 * 板组（conversation-column / conversation / terminal，消费对话
 * 区滑块变量），右栏 :not 链同步排除这三个面板值，三区域归属
 * 恢复各管各的容器。
 *
 * V9 变更（运行时实测选择器终版修正）：真机运行时检查确认——
 * 当前 ZCode 常驻 UI 中唯一带面板属性的面板是 workspace-main
 * （代码常量之值，覆盖中间对话+输入区主区域）；V8 枚举的对话列
 * 三面板组（conversation-column / conversation / terminal）仅在
 * 多面板视图（repo-wiki 等）展开时才挂载。V7/V8 症状根因：右栏
 * :not 反选把唯一的 workspace-main 捞走（右栏滑块实际控制了对
 * 话区），而对话区规则的 conversation-column 选择器在常驻 UI 无
 * 命中（对话滑块看似无效果）。V9 把对话区作用区扩为四面板组
 * （常驻 workspace-main + 多面板视图的对话列三面板组），右栏
 * :not 链同步排除这四个面板值；右栏"打开标签页"空态选择面板
 * 无面板属性（容器类 side-pane-open-tab-shell），单独并入右栏
 * 作用区，三区域归属彻底归位。
 *
 * V10 变更（文字可读性增强 + 氛围底可调）：--zbar-base-alpha 由
 * 固定常量升级为用户可调参数 base_alpha（variables.css 按参数渲染
 * 真值并随热重载即时生效，本文件中原有的 0.25 兜底写法保留，作
 * 旧 variables.css 的容错）；新增文字描边强度参数 text_shadow，
 * 消费变量 --zbar-text-shadow，文件末尾追加三区域主容器（与上方
 * 主区域背景规则同批选择器）的前景文字描边规则——深色主题补黑色
 * 描边、浅色主题补白色描边，把文字从过亮/过暗的壁纸里托出来；
 * 强度 0 时 alpha 为 0 描边自然不可见，默认观感与 V9 完全一致。
 * 既有 V9 选择器与规则一字未动。
 *
 * 生效前提：本文件 link 晚于 ZCode 自身样式（</head> 前注入，
 * 源顺序占优）；壁纸视频层与压暗遮罩层由 effects.js 挂载。
 * ============================================================ */

/* 根背景透明化兜底：ZCode body 本无全局背景，防止个别版本铺底色 */
html,
body {
  background: transparent !important;
}

/* ---- 浅色 token 半透明化（V5：全局底固定氛围值） ----
 * 引用 neutral 原色而非硬编码色值，跟随 ZCode 自身配色演进；
 * 全局底色 token 统一消费 variables.css 固定输出的氛围变量
 * --zbar-base-alpha（隐约透出壁纸的一层氛围底），与两个滑块
 * 彻底解绑：拖任一滑块都不会再牵连顶栏、右侧面板、卡片等
 * 全局底消费方。 */
:root {
  --color-background: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-background-alt: color-mix(
    in oklab,
    var(--color-neutral-100) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-panel: color-mix(
    in oklab,
    #fff calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  /* 侧栏专用 token（浅色取 neutral-100，同 ZCode 原始定义）：
     V5 起与其余全局底色 token 一并由固定氛围变量驱动，
     不再消费任何滑块变量 */
  --color-sidebar: color-mix(
    in oklab,
    var(--color-neutral-100) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
}

/* ---- 深色 token 半透明化 ----
 * .dark 与 :root 同特异性 (0,1,0)，本文件靠源顺序靠后胜出。 */
.dark {
  --color-background: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-background-alt: color-mix(
    in oklab,
    var(--color-neutral-800) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  --color-panel: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
  /* 侧栏专用 token（深色取 neutral-950，同 ZCode .dark 原始定义），
     同样由固定氛围变量驱动，不再消费任何滑块变量 */
  --color-sidebar: color-mix(
    in oklab,
    var(--color-neutral-950) calc(var(--zbar-base-alpha, 0.25) * 100%),
    transparent
  );
}

/* ---- V6 三区域分层说明 ----
 * 全局底色 token（background / background-alt / panel / sidebar）
 * 与滑块彻底解绑，统一由固定氛围变量提供"隐约透出壁纸"的一层
 * 氛围底；左栏 / 对话列 / 右栏三个滑块各自只驱动一个主容器的
 * 元素规则（见下方主区域规则），互不牵连。顶栏、内容卡片等
 * 其余区域消费全局 token，永远保持固定的氛围透明度。 */

/* ---- 主区域元素级半透明化（V6：三区域分层，滑块的唯一作用区） ----
 * 左栏与主内容区内的各面板原本自身无背景（透明，透出全局底色），
 * 全局 token 覆盖对它们无效，这里按元素直接刷半透明背景。V6 起按
 * 面板属性定位（react-resizable-panels 实例，无原生 DOM id），三区域
 * 各只消费自己的滑块变量、互不串味：
 * - 左栏侧栏容器 → 侧栏滑块变量；
 * - 对话区四面板组（V9：常驻主面板 workspace-main + 多面板视图
 *   才挂载的对话列面板组 conversation-column / conversation /
 *   terminal，子面板同带面板属性，须一并归入对话区）→ 对话区
 *   滑块变量；
 * - 右栏其余面板（面板属性 :not 反选排除上述四面板）与右栏"打开
 *   标签页"空态选择面板（无面板属性，按容器类定位）→ 右栏滑块
 *   变量，反选自动覆盖将来新增的面板，无需枚举。
 * 基准原色与全局底色一致（浅色 neutral-50、深色 neutral-900），保持
 * 区域间色彩连贯。深浅两套均加 !important 压过 ZCode 源样式。 */
#sidebar {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-sidebar-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark #sidebar {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-sidebar-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* V9：对话区作用区为四面板组——常驻主面板 workspace-main（当前
 * 常驻 UI 中唯一带面板属性的面板，覆盖中间对话+输入区主区域）+
 * 多面板视图展开时才挂载的对话列三面板组（外层 conversation-column
 * 与其内部子面板 conversation / terminal）。缺一则其余面板会被
 * 下方右栏 :not 反选捞走（归属串味） */
[data-pane-id="workspace-main"],
[data-pane-id="conversation-column"],
[data-pane-id="conversation"],
[data-pane-id="terminal"] {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-panel-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark [data-pane-id="workspace-main"],
html.dark [data-pane-id="conversation-column"],
html.dark [data-pane-id="conversation"],
html.dark [data-pane-id="terminal"] {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-panel-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* V9：:not 链排除对话区四面板组（常驻主面板 + 对话列三面板组），
 * 其余面板全部归右栏滑块；右栏"打开标签页"空态选择面板无面板
 * 属性，按其容器类并入右栏作用区 */
[data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
.side-pane-open-tab-shell {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-50) calc(var(--zbar-sidebar-right-opacity, 0) * 100%),
    transparent
  ) !important;
}

html.dark [data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
html.dark .side-pane-open-tab-shell {
  background-color: color-mix(
    in oklab,
    var(--color-neutral-900) calc(var(--zbar-sidebar-right-opacity, 0) * 100%),
    transparent
  ) !important;
}

/* ---- 文字可读性 ----
 * 深色壁纸上浅色文字发虚时，优先在 ZBar 面板调大遮罩强度
 * （--zbar-mask-strength，遮罩层由 effects.js 挂载并热重载更新），
 * 压暗壁纸提升前景对比度。
 * V10：已追加独立于遮罩的文字描边能力（强度变量 --zbar-text-shadow，
 * 用户可调），规则见文件末尾"文字可读性：壁纸过亮/过暗时给前景
 * 文字补描边"块。 */

/* ---- 文字可读性：壁纸过亮/过暗时给前景文字补描边（V10） ----
 * 壁纸以氛围底透出后，亮色壁纸配深色主题时白色文字、深色壁纸配
 * 浅色主题时深色文字都可能失去对比度；遮罩强度只能整体压暗/提亮，
 * 这里按主题方向对称补描边，把文字从背景里"托"出来：
 * - 深色主题（html.dark）给三区域主容器补黑色描边，浅色主题对称
 *   补白色描边；
 * - 强度全部消费 --zbar-text-shadow（用户可调 0~1，0=关闭；
 *   variables.css 按参数渲染真值并随热重载即时生效），alpha=0 时
 *   描边自然不可见，无需条件分支；
 * - 模糊半径随强度温和缩放（3px 起步，最高 +5px），低强度下边缘
 *   柔和不产生图标毛边；
 * - 选择器组与上方主区域背景规则同批（侧栏容器 + 对话区四面板组
 *   + 右栏其余面板与空态选择面板），描边设在容器上由文字继承，
 *   与背景三区域分层一一对应，不影响任何 V9 规则。 */
#sidebar,
[data-pane-id="workspace-main"],
[data-pane-id="conversation-column"],
[data-pane-id="conversation"],
[data-pane-id="terminal"],
[data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
.side-pane-open-tab-shell {
  text-shadow: 0 0 calc(3px + (var(--zbar-text-shadow, 0) * 5px))
    rgba(255, 255, 255, var(--zbar-text-shadow, 0));
}

html.dark #sidebar,
html.dark [data-pane-id="workspace-main"],
html.dark [data-pane-id="conversation-column"],
html.dark [data-pane-id="conversation"],
html.dark [data-pane-id="terminal"],
html.dark [data-pane-id]:not([data-pane-id="workspace-main"]):not([data-pane-id="conversation-column"]):not([data-pane-id="conversation"]):not([data-pane-id="terminal"]),
html.dark .side-pane-open-tab-shell {
  text-shadow: 0 0 calc(3px + (var(--zbar-text-shadow, 0) * 5px))
    rgba(0, 0, 0, var(--zbar-text-shadow, 0));
}

/* ---- V11：透出全视口应用壳容器 ----
 * 见文件头部 V11 变更说明。该容器铺满整窗且自带不透明底色，
 * 置透明后负 z-index 的壁纸视频/压暗遮罩层按原设计垫底透出；
 * 氛围底与面板半透明质感仍由上方既有 token/元素规则负责。 */
div.flex.h-dvh.flex-col.overflow-hidden {
  background: transparent !important;
}
"#;

/// 壁纸运行时脚本模板：读取 --zbar-* CSS 变量，在 body 上挂黑底占位层、
/// 壁纸媒体层（视频或图片，按壁纸扩展名二选一）与压暗遮罩层，并每秒
/// 热重载 variables.css——ZBar 面板改参数/换壁纸无需重启 ZCode 即时生效。
/// theme.css 为静态 link 不做周期热重载（模板升级场景由面板"重启 ZCode"
/// 按钮冷启动完全重载，见模板内 V5 说明）。
/// 版本化落盘（头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）。
pub const EFFECTS_JS: &str = r#"// ============================================================
// ZBAR-THEME-V5
// ZBar Agent 动态壁纸运行时（由 ZBar 落盘并随版本升级覆盖）
// ============================================================
// 读取 variables.css 注入的 --zbar-* CSS 变量，在 body 上创建：
//   - 黑底占位层（z-index:-2）：媒体就绪前垫底，防止加载期闪白
//   - 壁纸媒体层（z-index:-2）：按壁纸 URL 扩展名二选一——
//       .mp4/.webm/.mov → video（muted/loop/playsinline/autoplay，
//         object-fit:cover，canplay 后淡入）
//       .jpg/.jpeg/.png/.webp → img（object-fit:cover，onload 后淡入）
//     视频↔图片切换类型时移除旧元素重建；加载失败静默移除全部注入
//     元素，退回原生观感
//   - 压暗遮罩层（z-index:-1）：rgba(0,0,0,强度)，保证前景可读
// 滤镜（亮度/饱和/模糊）与遮罩对视频和图片同样生效；播放速率仅视频。
// 热重载：每 1000ms 强制重读 variables.css（link href 追加时间戳，
// Chromium 对 file:// 的常用强制重读手段），比对 --zbar-* 变量快照，
// 壁纸 URL / 滤镜三参 / 播放速率 / 遮罩强度变化即时应用。
// theme.css 不做周期热重载（V4 曾与 variables.css 同款每秒 cache-bust，
// V5 撤销）：样式表 href 变更会经历"旧样式表卸载失效 → 异步加载解析
// → 恢复"窗口，失效窗口内三区域背景/文字描边规则整体失效，背景闪回
// ZCode 原生底色，系统忙时窗口拉长到肉眼可见的周期性闪烁。theme.css
// 模板升级（版本化覆盖落盘）改由面板"重启 ZCode"按钮冷启动完全重载。
// 兼容：定位 variables.css 优先 link[data-zbar-variables]（新版注入行），
// 旧版注入行（无 data 属性）回退按 href 含 "variables.css" 匹配——
// 旧 asar 注入无需重装主题也能热重载。
// ============================================================
(function () {
  "use strict";

  if (!document.body) return;

  var VAR_NAMES = [
    "--zbar-wallpaper-url",
    "--zbar-wp-brightness",
    "--zbar-wp-saturate",
    "--zbar-wp-blur",
    "--zbar-mask-strength",
    "--zbar-playback-rate"
  ];

  function cssVar(name, fallback) {
    var v = getComputedStyle(document.documentElement).getPropertyValue(name);
    v = (v || "").trim();
    return v || fallback;
  }

  function num(value, fallback) {
    var n = parseFloat(value);
    return isFinite(n) ? n : fallback;
  }

  /* 从 url("file://…") 形式的变量值中提取纯地址再交给媒体元素 src */
  function urlOf(value) {
    var m = /^\s*url\(\s*(['"]?)(.*?)\1\s*\)\s*$/.exec(value || "");
    return m ? m[2] : "";
  }

  /* 定位注入的 variables.css link：新版注入行带 data-zbar-variables；
   * 旧版注入行（无 data 属性）靠 href 含 variables.css 兜底匹配 */
  function findVarsLink() {
    return (
      document.querySelector("link[data-zbar-variables]") ||
      document.querySelector('link[href*="variables.css"]')
    );
  }

  /* 壁纸类型：按 URL 扩展名判定（file_url 的 percent-encoding 不影响
   * 扩展名字符）；未知扩展按视频兜底（与旧版行为一致） */
  function kindOf(url) {
    var u = (url || "").toLowerCase().split("?")[0];
    if (/\.(mp4|webm|mov)$/.test(u)) return "video";
    if (/\.(jpe?g|png|webp)$/.test(u)) return "image";
    return "video";
  }

  /* ---- 注入的常驻层：黑底占位与压暗遮罩 ---- */
  var placeholder = document.createElement("div");
  placeholder.setAttribute("data-zbar-wallpaper", "placeholder");
  placeholder.style.cssText =
    "position:fixed;top:0;left:0;width:100%;height:100%;" +
    "z-index:-2;background:#000;pointer-events:none;";

  var mask = document.createElement("div");
  mask.setAttribute("data-zbar-wallpaper", "mask");
  mask.style.cssText =
    "position:fixed;top:0;left:0;width:100%;height:100%;" +
    "z-index:-1;pointer-events:none;";

  /* ---- 壁纸媒体层（video 或 img，按类型二选一） ---- */
  var media = null; /* 当前媒体元素 */
  var mediaKind = ""; /* "video" | "image" */
  var currentUrl = "";
  var dead = false; /* 当前壁纸加载失败标记：不自动重试同源，换源后复位 */
  var placeholderTimer = 0;

  /* 就绪（video canplay / img load）：应用速率（仅视频）、淡入并撤黑底 */
  function onReady() {
    if (mediaKind === "video") {
      applyRate();
      var p = media.play();
      if (p && p.catch) {
        p.catch(function () {
          /* 自动播放被拦截时保持首帧，不报错 */
        });
      }
    }
    media.style.opacity = "1"; /* CSS 过渡淡入（可重复调用） */
    /* 淡入完成后再撤黑底，全程不闪白 */
    if (placeholderTimer) clearTimeout(placeholderTimer);
    placeholderTimer = setTimeout(function () {
      if (placeholder.parentNode) placeholder.parentNode.removeChild(placeholder);
      placeholderTimer = 0;
    }, 500);
  }

  /* 加载失败：静默移除全部注入元素，页面退回原生观感；
   * 热重载轮询继续，壁纸指向换到新地址后自动重建 */
  function onDead() {
    unmount();
    dead = true;
  }

  function createVideo() {
    var v = document.createElement("video");
    v.setAttribute("data-zbar-wallpaper", "video");
    v.muted = true;
    v.loop = true;
    v.playsInline = true;
    v.autoplay = true;
    v.style.cssText =
      "position:fixed;top:0;left:0;width:100%;height:100%;" +
      "object-fit:cover;z-index:-2;pointer-events:none;" +
      "opacity:0;transition:opacity .35s ease;";
    v.addEventListener("canplay", onReady);
    v.addEventListener("error", onDead);
    return v;
  }

  function createImage() {
    var i = document.createElement("img");
    i.setAttribute("data-zbar-wallpaper", "image");
    i.alt = "";
    i.style.cssText =
      "position:fixed;top:0;left:0;width:100%;height:100%;" +
      "object-fit:cover;z-index:-2;pointer-events:none;" +
      "opacity:0;transition:opacity .35s ease;";
    i.addEventListener("load", onReady);
    i.addEventListener("error", onDead);
    return i;
  }

  function mounted() {
    return !!(media && media.parentNode);
  }

  /* 挂载三层：媒体层先入，占位层插到媒体层之前（同 z-index:-2 时
   * DOM 靠后者居上，占位层必须先于媒体层插入才会被媒体盖住） */
  function mount() {
    if (!media) return;
    if (!media.parentNode) document.body.appendChild(media);
    if (!placeholder.parentNode) {
      document.body.insertBefore(placeholder, media);
    }
    if (!mask.parentNode) document.body.appendChild(mask);
  }

  function unmount() {
    if (placeholder.parentNode) placeholder.parentNode.removeChild(placeholder);
    if (media && media.parentNode) media.parentNode.removeChild(media);
    if (mask.parentNode) mask.parentNode.removeChild(mask);
  }

  /* ---- 变量应用 ---- */
  function applyFilter() {
    var b = num(cssVar("--zbar-wp-brightness", "1.1"), 1.1);
    var s = num(cssVar("--zbar-wp-saturate", "1.4"), 1.4);
    var l = num(cssVar("--zbar-wp-blur", "0"), 0);
    if (media) {
      media.style.filter =
        "brightness(" + b + ") saturate(" + s + ") blur(" + l + "px)";
    }
  }

  function applyRate() {
    /* 播放速率仅视频有意义，图片直接跳过 */
    if (mediaKind !== "video" || !media) return;
    var r = num(cssVar("--zbar-playback-rate", "1"), 1);
    try {
      media.playbackRate = r;
    } catch (e) {
      /* 个别编码下设置速率可能受限，忽略即可 */
    }
  }

  function applyMask() {
    var m = num(cssVar("--zbar-mask-strength", "0"), 0);
    mask.style.background = "rgba(0,0,0," + m + ")";
  }

  /* ---- 壁纸切换（首建与热重载换源共用） ---- */
  function setWallpaper(url) {
    var kind = kindOf(url);
    currentUrl = url;
    dead = false;
    /* 类型变化（视频↔图片）时移除旧元素重建，避免元素属性串味 */
    if (media && mediaKind !== kind) {
      if (media.parentNode) media.parentNode.removeChild(media);
      media = null;
      mediaKind = "";
    }
    if (!media) {
      mediaKind = kind;
      media = kind === "image" ? createImage() : createVideo();
    }
    mount();
    media.style.opacity = "0"; /* 重置为占位态，就绪后再淡入 */
    media.src = url;
    if (kind === "video") media.load();
    applyFilter();
    applyMask();
  }

  /* ---- 热重载：每 1000ms 强制重读 variables.css 并比对快照 ---- */
  var snapshot = {};

  function snapshotOf() {
    var cs = getComputedStyle(document.documentElement);
    var o = {};
    for (var i = 0; i < VAR_NAMES.length; i++) {
      o[VAR_NAMES[i]] = (cs.getPropertyValue(VAR_NAMES[i]) || "").trim();
    }
    return o;
  }

  function sameSnapshot(a, b) {
    for (var i = 0; i < VAR_NAMES.length; i++) {
      if (a[VAR_NAMES[i]] !== b[VAR_NAMES[i]]) return false;
    }
    return true;
  }

  /* href 追加时间戳强制重读文件；percent-encoded 路径不含裸 "?"，
   * split 取基址安全 */
  function reloadVarsLink() {
    var link = findVarsLink();
    if (!link) return;
    var base = (link.getAttribute("href") || "").split("?")[0];
    link.setAttribute("href", base + "?t=" + Date.now());
  }

  function poll() {
    try {
      /* 注入层被页面意外清掉时自愈（加载失败 dead 态除外） */
      if (!dead && currentUrl && !mounted()) {
        setWallpaper(currentUrl);
        return;
      }
      /* 先取快照，后重读 variables.css：reloadVarsLink() 改 href 会让
       * 样式表立即进入"卸载失效 → 异步加载解析 → 恢复"窗口。若沿用
       * 旧顺序（先重读后快照），快照正好撞上本轮重载的失效窗口读到
       * 空值；先取快照则读到的恒为上一轮重载稳定后的值（距本轮 href
       * 变更已隔一个轮询周期）。 */
      var now = snapshotOf();
      /* 空值防御：任一变量读到空串，说明 variables.css 仍处于上一轮
       * href 变更的重载窗口（旧样式表已卸载、新规则未解析完），或页面
       * 初载样式表尚未首次解析完成——本轮快照不可信，视为失效窗口直接
       * 返回：不 diff、不应用滤镜/遮罩、不切壁纸，避免把全部变量误判为
       * "被重置为默认值"而清掉用户参数。正常落盘的 variables.css 恒定
       * 渲染全部变量，空串只会出现在失效窗口，判定无歧义。 */
      for (var i = 0; i < VAR_NAMES.length; i++) {
        if (now[VAR_NAMES[i]] === "") return;
      }
      reloadVarsLink();
      if (sameSnapshot(snapshot, now)) return;
      snapshot = now;
      var url = urlOf(now["--zbar-wallpaper-url"]);
      if (url && url !== currentUrl) {
        setWallpaper(url); /* 内部已应用滤镜与遮罩 */
      } else {
        applyFilter();
        applyRate();
        applyMask();
      }
    } catch (e) {
      /* 单轮失败静默，下一轮重试 */
    }
  }

  poll(); /* 立即建层，不等首个周期 */
  setInterval(poll, 1000);
})();
"#;

/// 对话页用量统计条模板：在每轮对话（section[data-turn-id]，值实为
/// msg_ 前缀的用户消息 id）同轮 DOM 顺序最后一个单元节点末尾渲染一行
/// 小字统计条（↑ 非缓存输入 ↓ 输出 ⟲ 缓存读 · × 请求数 · 输出速度 ·
/// TTFT 首字延迟，V5 格式）；并在对话输入框上方固定悬浮会话级实时
/// 统计条（Σ 会话累计）。V6 起轮次完成后（turn_usage 落库）显示最终
/// 真实值，生成过程中经 runs 数组（model_usage 已落库、turn_usage 尚未
/// 写入的进行中轮实时聚合，usage_feed 每 2 秒随 turns 一并导出）+ DOM
/// 流式输出估算实时跳动（Claude Code CLI 式）；V8 起活动轮判定为 DOM
/// 驱动（会话 DOM 最后 umid 节点不在 index 即活动轮），首笔模型请求
/// 完成前的启动窗口也即时渲染。V10 起三态（启动窗口/live/完成）统一为
/// 同一固定结构、字段等宽补位，任何状态只更新数值不改变结构（行格式
/// 与枯萎清理见模板头 V10 变更说明）。V13 起会话条定位为零测量的布局
/// 方案：ensureStyle 给输入区容器 .chat-composer-region 注入 relative +
/// padding-top:26px 顶部留白，会话条（absolute top:4px 居中）由
/// renderSessionBar 幂等挂载进容器、住进留白内，输入框单行/多行切换、
/// 窗口缩放均随文档流自动跟随，零坐标测量、无 resize 监听；region 缺
/// 失时退回旧路径（挂 body + fixed + SESSION_BAR_BOTTOM_PX 96px 兜底）。
/// V12 的动态测量方案（getBoundingClientRect().top → fixed bottom）因
/// 实机会压住输入框内第一行文字废弃（测量目标与可见输入卡片边缘不一
/// 致 + 输入框单行/多行切换的时序窗口）。
/// 数据源为本目录下 usage-data.js（键名契约见 usage_feed 模块头；每轮
/// 条匹配键为 umid 字段）。版本化落盘（头部 ZBAR-THEME-V 标记，见
/// store::ensure_versioned_template）。风格与 effects.js 同款：自愈、
/// 静默失败、空值防御；DOM 选择器集中在头部常量，便于实机比对调整。
pub const USAGE_JS: &str = r#"// ============================================================
// ZBAR-THEME-V19
// ZBar Agent 对话页用量统计条（由 ZBar 落盘并随版本升级覆盖）
// ============================================================
// V19 变更（新增每轮统计条开关参数 usage_turn_bar，默认开启）：renderAll
//   第二遍渲染前统一读 --zbar-usage-turn-bar（variables.css 渲染 1/0，
//   变量缺失视为开启，兼容旧 variables.css），关闭时对全部轮节点
//   removeRow 并跳过 renderOne（removeRow 幂等，已渲染行随关闭清掉、
//   开启后自动恢复）；估算管线 syncDyn 与会话条不受影响。
// V18 变更（修复新建任务后（空会话）会话累计条停留在上一个会话数据：
//   renderAll 开头无 [data-turn-id] 节点时直接早退，renderSessionBar
//   永不执行，V17 修复的容器判定本身正确但该分支根本到不了会话条渲
//   染，条停留在上一个会话的累计值永不消失。实机复现：新任务容器
//   data-session-id="draft" 可见且含焦点）：
//   a) renderAll 空轮分支在 stopDyn 之后补 removeBar()：会话内无任何
//      轮节点即移除会话条。其余逻辑零改动。
// V17 变更（修复新建任务后会话累计条不消失、数据不重置：多会话保活
//   下旧会话容器在任务切换后仍挂载在 DOM 且通常排在前面，原
//   currentSessionId 用 querySelector 首中即取，读到保活的旧会话 id，
//   条继续渲染旧会话累计，应读到新任务的 draft/无数据并随之消失）：
//   a) 新增 visibleEl：元素当前是否可见（保活面板隐藏时 rect 归零或
//      display:none）。
//   b) currentSessionId 重写为容器遍历两级优先：包含
//      document.activeElement 的容器（用户正在输入的会话）优先，其次
//      第一个可见容器；锚点内优先、无候选再全文档。都拿不到返回空
//      串，renderSessionBar 既有逻辑随之 removeBar。
//   c) composer 挂载点新增 pickComposerRegion：可见且属于当前会话容
//      器（closest 命中会话容器属性值）优先，其次第一个可见的，都无
//      返回 null 走既有 body 兜底分支。渲染与数据管线零改动。
// V16 变更（整体移除每轮统计行（完成态/进行中/启动窗口）的鼠标悬浮
//   title 提示。用户反馈：悬浮信息无必要，且原生 title 提示框样式丑）：
//   a) 删除三态行的 title 赋值，以及仅为其服务的明细构造函数与口径
//      常量（悬浮明细文案随之下线，标识符全文零残留）。行可见内容与
//      数据管线零改动；fmtSeconds 因 lineOf 的 TTFT 字段仍在使用而
//      保留。
// V15 变更（会话累计条显示格式调整。用户反馈：动态段 ⋯ 前缀与流式
//   估算 ↓ ~ 前缀语义不明，且 Σ 段缺少会话总量数字）：
//   a) Σ 段新增会话总 Token（tsum = 输入+输出+缓存读之和，真实数据
//      不含估算），原 Σ ↑ 输入改为独立 ↑ 明细段。
//   b) 速度段去掉 ⋯ 前缀（与每轮条速度段一致）。
//   c) 流式估算段 ↓ ~ 改 ≈ 前缀（生成中未落库的输出估算，不计入
//      累计）。每轮条（renderOne/live 行）格式零改动。
// V14 变更（修复 V13 遗漏 SEL_COMPOSER 定义：renderSessionBar 挂载块
//   每次执行 querySelector(SEL_COMPOSER) 读到未声明变量抛
//   ReferenceError，被挂载 catch 静默吞掉，会话条永远走 body + fixed
//   bottom 兜底、压住输入区内部的引用条/附件行）：
//   a) 常量区补回 SEL_COMPOSER 定义（值为 ".chat-composer-region"）。
//   b) 挂载 catch 增加一次性 console.warn（mountWarned 标志），便于实
//      机 DevTools 排查兜底态；渲染与数据管线零改动。
// V13 变更（会话条 DOM 挂载进输入区容器 + CSS 顶部留白定位，废弃动态
//   测量。实机缺陷：V12 的"测 region.top → fixed bottom 计算"会压住
//   输入框内第一行文字——测量目标（region 容器上沿）与可见输入卡片边
//   缘不一致，且输入框单行/多行切换存在时序窗口，测量值滞后于实际布
//   局；DevTools 调试通道本次不可用，直接改为无测量的布局方案）：
//   a) ensureStyle 注入 .chat-composer-region{position:relative
//      !important;padding-top:26px !important}：输入区容器顶部留白
//      26px，会话条住进留白内。relative 风险说明：region 类名自带
//      z-20（设计上已是定位元素），补 relative 通常无副作用；若
//      ZCode 后续改用其它定位方案，本 !important 规则有覆盖风险，
//      实机回访时留意。
//   b) 会话条样式 fixed/bottom → absolute（top:4px;left:50%;
//      translateX(-50%) 居中）：输入框单行/多行增高、窗口缩放时留白
//      随文档流自动跟随，条恒在容器顶部，零坐标测量、零事件监听。
//   c) 挂载进容器：renderSessionBar 幂等迁移（bar.parentElement !==
//      region 才 appendChild；React 不主动清理非自身创建的子节点，
//      此前消息 section 内手插行长期存活已验证；region 被重建时条
//      断连，ensureBar 的 isConnected 检查自愈重建）。
//   d) region 缺失退回旧路径：挂 body + fixed + bottom:
//      SESSION_BAR_BOTTOM_PX（96 兜底），经 data-zbar-usage-session-
//      fixed 属性选择器切换样式，迁回 region 时移除标记自动还原。
//   e) 删除 getBoundingClientRect 测量、SESSION_BAR_ABOVE_PX 常量、
//      window resize 监听（absolute 随文档流自适应，无需重定位）。
//      渲染与数据管线零改动。
// V12 变更（会话条动态测量定位到输入框上沿之上、还原输入框位置、
//   resize 自适应。用户反馈：V11 的"输入框上移 22px + 会话条贴窗底"
//   观感差——统计条贴死窗口边缘、与 ZCode 留白风格不协调）：
//   改回 CLI 惯例——输入框还原原位（删除 V11 的
//   .chat-composer-region padding-bottom 规则与 COMPOSER_GAP_PX 常量），
//   会话条每次渲染动态测量输入区容器（SEL_COMPOSER =
//   .chat-composer-region，常量保留改为定位测量用）
//   getBoundingClientRect().top，style.bottom =
//   window.innerHeight - top + SESSION_BAR_ABOVE_PX（条底距输入区上沿
//   6px），彻底解决 V5 写死 bottom:96 落入输入框内部的问题（输入框
//   实际顶部距底约 180px，写死值不可靠）；region 不存在 / 测量异常时
//   退回固定 SESSION_BAR_BOTTOM_PX = 96（兜底值）。初始化追加
//   window resize 监听（scheduleRender），窗口缩放后下帧自动重定位。
//   渲染与数据管线零改动。
// V10 变更（统计条显示稳定性：结构整体固定只更新数值）：
//   - 每轮条三态（启动窗口/live/完成）统一为同一固定结构，任何状态都
//     渲染全部字段位、只更新数值。原三态各一种结构（启动窗口"逐段省
//     略"极简行 / live 数字段 + 行尾 … 标记 / 完成完整格式）导致段数
//     与左右宽度随状态持续变化，观感差。统一格式（barLineOf）：
//     "↑ <in> ↓ <out> ⟲ <cr> · × <req> · <speed> t/s · TTFT <ttft>"。
//     启动窗口态：数字位显示 0 / 估算值（↓ 为估算输出），TTFT 位显示
//     "–"（进行中未定）；live 态：真实聚合（含 sub）+ 估算叠加，TTFT
//     位 "–"；完成态：最终值 + TTFT 数值。估算标识 "~" 改为固定占位
//     字符位：↓ 字段值固定 1 个前缀字符（live/启动 "~"、完成为空格），
//     宽度恒定。原行尾 "…" 进行中标记删除（TTFT 位 "–" 已表达进行中，
//     title 口径说明保留）；启动窗口极简行的省略逻辑删除，合并进统一
//     格式函数。
//   - 数字等宽补位（等宽字体 + tabular-nums 下各字段占位恒定，整行宽
//     度恒定）：token 值经 fmtTokens 恒定 5 字符（"  998"/" 1.2k"/
//     "10.5M"，超 999.9M 自然溢出）；req padStart(3)；速度 toFixed(1)
//     后 padStart(4)，dur 缺失（老库）显示 4 字符占位 "  - "；TTFT
//     padStart(4)（"x.xs" 恒 4 字符），进行中/缺失显示 "–"。title
//     hover 明细不受等宽约束（补位结果在 title 消费端 trim）。
//   - 会话条动态段固定："⋯ <speed> t/s · ↓ ~<est>" 两段永远显示（idle
//     无活动轮时速度 0.0、估算 0），不再按有无值省略，Σ 行整体宽度恒
//     定；Σ 数字段同步补位（req padStart(3)，token 经 fmtTokens）。
//   - 顺带修复已完成子代理面板的残留占位：已完成的子代理轮（值已并入
//     主轮 sub，turns/runs 永无该 umid 行）此前在子代理详情面板永久
//     显示启动占位行。新增"枯萎"判定（STALE_MS = 90000）：活动轮目标
//     节点文本连续 90 秒无增长且该 umid 始终不在 index/runIndex →
//     移除该行并从活动轮目标中移除；目标文本再变化（恢复输出/虚拟列
//     表重挂）时消费端失效记录重新评估，无害。
// V9 变更（子代理消耗实时化：主轮条/会话条随子代理消耗动态更新 + 子代
//   理详情面板自身统计）。实机核实：子代理详情面板与主对话同 document
//   （无 iframe），面板有自己的 [data-session-id] 容器（值为子代理会
//   话 id sess_subagent_*）与 [data-turn-id] 节点（值为子代理会话自己
//   的 umid，与数据 runs 子代理行完全一致），面板默认关闭、点开才挂
//   载；数据侧 runs 新增主会话行 sub 聚合（并入的子代理实时消耗，含
//   游离子代理完成轮）与子代理行 m:1 防双计标记（usage-data.js 仍为
//   v2 附加字段，向后兼容）。V8 的三个缺陷与修复：
//   - 扫描范围 document 级：V8 的 renderAll 与活动轮判定限定在
//     workspace-main 锚点内，扫不到锚点外的子代理面板。V9 改
//     document.querySelectorAll（35 节点量级，性能无虞），锚点仅保留
//     给会话条定位。子代理面板内的每轮条随 document 扫描按子代理行
//     umid 命中渲染 live 条（子轮完成后 runs 行退出、turns 无子代理
//     行，条自然消失——消耗并入主轮完成值，面板条不残留）。
//   - 多容器活动轮（findLiveNodes 取代 findLiveNode）：遍历所有
//     [data-session-id] 容器，每容器取 DOM 顺序最后一个 data-turn-id
//     节点，umid 不在 index（完成轮）即为该会话的活动轮——启动窗口
//     与 runs 阶段同一节点（V8 口径不变，判定仍不依赖 runs 数据）。
//     返回 Map：会话 id → 活动轮节点；主对话与并行多个子代理面板各
//     有各的活动轮，互不影响。
//   - 主轮条 live 态显示 sub：runIndex 命中的行若带 sub（数据侧并入
//     的子代理实时聚合），数字段按合计显示（↑↓⟲× 加子代理部分，流
//     式估算仍只叠加本会话活动轮节点），title 分解"含子代理 n 轮：…"
//     （沿用完成态 title 的分解格式）——子代理消耗实时反映在主对话每
//     轮条上。
//   - 会话条 Σ 口径修正：runs 行合计跳过 m:1 的子代理行（其值已并入
//     主轮行 sub，随主轮行一并计入——原 V8 按 psess 裸命中，子代理行
//     与主轮行并存时双计、主轮行缺失时又丢游离完成轮）；父会话暂无
//     主轮行的子代理行不带 m，仍按 psess 并入，无缝衔接。会话条仍只
//     渲染主窗口（锚点定位 + 优先取锚点内会话容器 id，子代理面板无
//     输入区不渲染会话条）。
//   - 估算器多目标（dyn.targets：sess → {node, 基准长度, 窗口样本}）：
//     单一 200ms 定时器统一采样驱动，各会话活动轮独立差分估算互不干
//     扰；目标的建立/换轮重建/清理统一由 renderAll 的 syncDyn 依活动
//     轮判定结果处理（轮完成 → 活动轮消失 → 目标移除 → 定时器空转
//     自停），关闭会话条不再连带停估算（live 每轮条仍需估算叠加）。
// V8 变更（启动窗口实时渲染：活动轮判定从数据驱动改为 DOM 驱动）：
//   实机缺陷：发消息后 agent 已开始思考输出，但每轮统计条不显示、会话
//   累计条一动不动，直到第一笔模型请求完成后才有内容。根因：V6/V7 的
//   实时渲染以数据驱动——live 每轮条与估算目标（findLiveNode）都依赖
//   runIndex（runs = model_usage 已完成请求的聚合），而轮次开始到第一
//   笔请求完成之间（思考阶段可达几十秒）model_usage 无行 → runs 无该
//   轮 → live 条不渲染、估算无目标、会话条 sessionRunTotals 为空——
//   启动窗口全空白；data-running 兜底实机不可靠（该属性取值存疑）。
//   V8 活动轮判定改为 DOM 驱动：会话内 DOM 顺序最后一个 data-turn-id
//   （umid）节点，其 umid 不在 index（完成轮）即为活动轮——既不在
//   index 也不在 runIndex = 启动窗口活动轮（消息发出节点即在 DOM，无
//   需任何数据库数据）；在 runIndex = runs 阶段活动轮（同一节点继续
//   估算叠加）。
//   - 每轮条（启动窗口轮）：立即渲染动态段 + 进行中标记。真实部分全 0
//     无意义，不渲染数字段，格式为 "⋯ X.X t/s · ↓ ~X …"（速度与估算
//     输出来自 DOM 估算，思考/正文文本增长都计入；title 说明"生成中：
//     第一笔请求完成后显示真实用量"；样本不足逐段省略，最少为 "…"）。
//     runIndex 命中后（首笔请求完成，2 秒内）自动切换为 live 完整格式
//     （真实聚合 + 估算叠加）。data-running 等待态渲染分支删除——判定
//     不再需要它，data-running 不再是任何渲染路径的必要条件（仅保留在
//     MutationObserver attributeFilter 作额外刷新信号）。
//   - 估算目标统一（findLiveNode 重构）：活动轮 = 会话 DOM 最后一个不
//     在 index 的 umid 节点，启动窗口与 runs 阶段同一节点；删除对 runs
//     的目标依赖与 data-running 依赖；会话限定经最近 data-session-id
//     容器收敛（防同页多会话 DOM 相邻串扰）。
//   - 会话累计条：Σ 真实部分照旧（完成轮 + runs），启动窗口轮的估算
//     输出计入动态段 ↓ ~X（不再叠加进 Σ ↓ 真实数字，避免估算污染
//     累计）；放弃渲染条件追加"无活动轮"——发消息即出现 ⋯ t/s ·
//     ↓ ~X 跳动；runs 出现后动态段继续叠加当前流式请求。
//   - 轮次完成的切换不变：turn_usage 行出现 → 启动窗口/live 态被完成
//     态替换；估算器在活动轮 umid 进入 index 时重置（findLiveNode 不
//     再返回该节点 → stopDyn，现状收尾路径）。多会话各自页面实例独立
//     渲染，活动轮判定限定当前会话容器，互不影响。
// V7 变更：请求次数图标 ⟳ → ×（原 ⟳ 与缓存读 ⟲ 仅箭头方向之差，过于
//   相似易混淆；× 读作"共 N 次"，缓存读 ⟲ 保持不变）
// V6 变更（生成过程实时跳动）：
//   - 数据源 usage-data.js 追加 runs 数组（v2 契约不变，runs 为附加
//     字段，旧脚本忽略未知字段平滑兼容）：model_usage 已落库、
//     turn_usage 尚未写入的进行中轮实时聚合——工具循环每完成一步模型
//     请求即落一行 model_usage（2 秒轮询内可见），无需等整轮结束。行
//     形如 { umid, sess, psess, in/out/cr/cw/rt, req, start }（psess
//     仅子代理会话有值，指回父会话）。
//   - 每轮条渲染优先级：完成数据（index 命中，最终真实值，V2 起逻辑
//     不变）> 进行中 run（runIndex 命中）> data-running 等待态 > 不
//     渲染。进行中轮行 = run 真实聚合 + 当前流式输出估算叠加到 ↓ +
//     估算速度段 + 尾缀 "…"（title 说明口径：已完成请求为真实值，当
//     前流式输出为估算）。轮完成后 turn_usage 行在 2 秒内到达，run 行
//     同步退出 runs，渲染自然切最终真实值——切换时数字可能小幅修正
//     （最后一笔进行中请求完成后才计入 turn_usage，估算部分被真实值
//     替换），属预期。
//   - 会话条 Σ = 完成轮合计 + runs 中 sess 或 psess 命中当前会话的行
//     合计 + DOM 流式估算，生成期间持续跳动；新会话首轮（无任何完成
//     轮）也即时显示。修复 V5 动态段不生效的两个根因：
//     a) renderSessionBar 在 sessionTotals 为 null（会话尚无完成轮，
//        新会话首轮必现）时提前返回，动态定时器从未启动——生成期间
//        会话条完全静止；V6 改为完成合计与 run 合计均为空才放弃。
//     b) 动态目标节点检测依赖 data-running="true"，实机不可靠（等待
//        态同样受影响，见用户反馈"完成后才显示数字"）。V6 目标节点改
//        由 runIndex 数据驱动（run 命中的 DOM 节点，2 秒内必达），
//        data-running 仅作数据未达头 2 秒的兜底。
//   - 更新节奏：真实部分随数据轮询（2 秒），流式估算部分随动态定时器
//     （200ms，textContent.length 差分 × TOKEN_CHARS 折算 + 滑动窗口
//     求速），二者合并渲染；等宽 + tabular-nums 防跳动宽度抖动（V5
//     沿用）。
// V5 变更（行格式图标化 + 会话级实时统计条）：
//   - 每轮行格式调整（数据与渲染管线零改动，仅显示层）："N req" →
//     "× N"（× = 模型请求次数）、"tok/s" → "t/s"、"首字 X.Xs" →
//     "TTFT X.Xs"（time to first token 公认缩写）。新格式示例：
//     ↑ 1.2k ↓ 3.4k ⟲ 89k · × 25 · 90.0 t/s · TTFT 5.8s
//   - 新增会话级实时统计条（renderSessionBar，独立管线）：fixed 悬浮于
//     对话输入框上方（锚点 workspace-main，bottom 由 SESSION_BAR_BOTTOM_PX
//     常量控制便于实机调参），显示当前会话全部轮次的真实累计
//     （Σ ↑非缓存输入 ↓输出 ⟲缓存读 ×请求数，按 sess 过滤 turns 聚合，
//     已含并入的子代理部分），随 2 秒数据轮询刷新。找不到锚点/会话 id/
//     会话无任何轮次（draft 空会话）时不渲染，静默降级。
//   - 流式生成动态段：轮进行中（data-running="true"）时在累计后追加
//     "⋯ X.X t/s · ↓ ~Y"（~ 前缀标识估算值）。估算来源：动态定时器每
//     DYN_TICK_MS 采样 running 轮节点 textContent.length 差分，按
//     TOKEN_CHARS（3.5 字符/token，中英混合经验值）折算 token，并在
//     SPEED_WINDOW_MS 滑动窗口内差分求速（防抖动）；不做文本统计专用
//     observer（复用现有 scheduleRender 监听 + 定时器采样，性能优先）。
//     轮完成（running 消失）即清定时器并移除估算段，累计值在 2 秒内经
//     数据轮询切回数据库真实值（直接切换不平滑过渡，真实优先）。
//   - 会话条开关：ThemeParams.usage_session_bar 经 variables.css 的
//     --zbar-usage-session-bar（1/0）透出（effects.js 每秒热重载，改
//     开关约 1 秒生效）；变量缺失（旧 variables.css）时视为开启（默认
//     true）。字号/不透明度复用 --zbar-usage-font-size/opacity。
// V4 变更（样式参数化）：统计条字号与不透明度改为消费 variables.css 的
//   --zbar-usage-font-size / --zbar-usage-opacity（皮肤页新增"用量统计条"
//   滑块可调，variables.css 由 effects.js 每秒热重载，拖动约 1 秒生效，
//   无需重启）；模板内仅保留 V3 写死值（10px / .55）兜底。原 font 简写
//   `400 10px/1.5` 拆为 font-family/size/weight/line-height 独立声明以
//   参数化字号；等待态低透明由正常态乘 0.636 系数得出（默认
//   0.55×0.636≈0.35，与 V3 写死 .35 观感一致）。渲染/匹配/死锁防护
//   逻辑零改动。
// V3 变更（实机缺陷修复）：scheduleRender 的 rAF 回调改为 try/finally
//   结构——scheduled 复位移入 finally，renderAll 抛异常不再导致 scheduled
//   永久为 true 卡死渲染管线；另加 15 秒低频兜底渲染（不经过 scheduled
//   检查直接调用 renderAll），未来出现新的意外状态也能在一个周期内自愈。
// V2 变更（实机缺陷修复）：实测 DOM data-turn-id 的值是 msg_ 前缀的
//   用户消息 id（等于主库 turn_usage.user_message_id）而非 turn id，
//   匹配键从 turn 改为数据 v2 新增的 umid 字段；同一轮被虚拟列表拆成
//   多个单元节点时，统计条只渲染在 DOM 顺序最后一个节点（回复结束处），
//   其余节点旧行移除，"最后一个"随节点集合变化正确迁移；数据 ts 与
//   上次相同时跳过索引重建与重渲染（常态每 2 秒重载零成本）。
// 数据源：ZBar 后台任务从 ZCode 主库 turn_usage（官方每轮聚合表，轮次
// 完成时才落库）+ model_usage（每步模型请求完成即落一行，V6 起取其
// 进行中轮）导出的本目录 usage-data.js，内容形如：
//   window.__ZBAR_USAGE__ = { v:2, ts:<最后数据变化时刻ms>, turns:[{
//     umid: "msg_xxx"   用户消息 id（turn_usage.user_message_id，与 DOM
//                       data-turn-id 同值，即本脚本匹配键；可 null，
//                       null 轮无法与 DOM 匹配，仅透出）
//     turn: "turn_xxx"  轮 id（保留透出，不用于 DOM 匹配）
//     sess / status / start / end   会话、状态、起止毫秒
//     in / out / cr / cw / rt       输入(含缓存读)/输出/缓存读/缓存写/推理
//                                   ——均已并入该轮覆盖的子代理聚合
//     req / retry / tool            模型请求数 / 重试数 / 工具调用数（同上）
//     dur / ttft                    主轮自身总耗时 / 首字延迟毫秒（可 null）
//     sub: { n, req, in, out, cr, cw, rt }  并入的子代理聚合（可 null）
//     models: "GLM-5.3,..."         该轮模型（去重逗号拼接，含子代理）
//   }],
//     runs: [{           进行中轮实时聚合（V6 附加字段；空数组也输出）
//     umid / sess        匹配键与会话（同上；子代理轮 umid 指向子代理
//                        会话自己的消息，V9 起子代理详情面板按它匹配）
//     psess              父会话 id（仅子代理会话有值，主会话 null）
//     m                  V9：子代理行且父会话存在进行中主轮行时为 1
//                        ——数值已并入主轮行 sub，会话累计跳过防双计
//     sub                V9：主会话行并入的子代理实时聚合（并行子代理
//                        runs + 游离子代理完成轮），每轮条与会话条按
//                        合计口径显示，title 分解明细
//     in / out / cr / cw / rt / req  该轮已完成请求的聚合
//     start              首个请求开始毫秒
//   }] }
// 导出窗口：turns 最近 7 天、至多 3000 轮；runs 近 10 分钟内有请求的
//   进行中轮（turn_usage 已有行的完成轮不进 runs）。
// 展示口径（与 ZBar 面板 db.rs gen_window_expr 一致，保守取小值）：
//   ↑ = in − cr（非缓存输入）↓ = out ⟲ = cr × = req；速度 =
//   (out+sub.out)×1000/gen，gen = dur − ttft（≥1ms）；ttft 缺失时
//   gen = dur；ttft ≥ 90% dur（整块下发）时 gen = ttft；dur 缺失速度
//   位显示占位（V10 固定结构）；TTFT = ttft（缺失/进行中显示 "–"）。
// 行格式（V10 起三态统一固定结构）：每轮条任何状态都渲染
//   "↑ <in> ↓ <out> ⟲ <cr> · × <req> · <speed> t/s · TTFT <ttft>"，
//   各字段等宽补位（token 5 字符 / req 3 字符 / 速度与 TTFT 各 4 字
//   符；↓ 前固定 1 字符估算前缀位，进行中 "~"、完成空格），等宽字体 +
//   tabular-nums 下整行宽度恒定，只更新字段数值不改变结构。
// 行为：每 2 秒以 script 标签重载 usage-data.js（先删旧再插新，加载失败
//   保留上次数据；页面隐藏降频 10 秒）；v !== 2 视为无效数据走静默路径
//   （保留上次数据）；ts 与上次相同则跳过索引重建与重渲染。
//   MutationObserver + 首次全量扫描定位 [data-turn-id]（V9 起扫描
//   document 级，覆盖主对话与子代理详情面板）；每轮条按优先级渲染：
//   完成数据按 umid 命中（同 umid 多节点只在 DOM 顺序最后一个节点出）
//   > 进行中 runIndex 命中（真实聚合 + 并入的子代理 sub 合计 + 流式
//   估算）> 启动窗口活动轮（DOM 驱动判定：本会话容器 DOM 最后 umid
//   节点且数据未达，统一固定结构渲染 0 / 估算值）> 不渲染（避免脏数
//   据；V10 起枯萎目标维持移除，见 STALE_MS）；虚拟列表
//   回收重挂按 umid 幂等重渲，节点消失不残留状态。renderAll 尾部同步
//   驱动会话级实时统计条（活动轮判定在 renderAll 统一计算一次，每轮
//   条与会话条共用），动态运行期另由 200ms 估算定时器（多会话目标统
//   一采样）自驱动全量重渲，15 秒兜底渲染覆盖二者的自愈。
// ============================================================
(function () {
  "use strict";

  if (!document.body) return;

  /* ---- 选择器与常量（集中此处，便于实机比对调整） ---- */
  /* 对话区锚点：V9 起仅用于会话条定位与会话 id 收敛（每轮条/活动轮
   * 判定改 document 级扫描，覆盖锚点外的子代理详情面板） */
  var SEL_PANE_ANCHOR = '[data-pane-id="workspace-main"]';
  var SEL_TURN = "[data-turn-id]"; /* 每轮对话节点 */
  var ATTR_TURN_ID = "data-turn-id";
  var ATTR_RUNNING = "data-running"; /* 轮进行中属性（实机取值存疑）：
    V8 起不参与任何活动轮/渲染判定，仅保留在 MutationObserver
    attributeFilter 作为属性变化的额外刷新信号 */
  var ATTR_ROW = "data-zbar-usage-row"; /* 统计条标记（防重复） */
  var STYLE_ID = "zbar-usage-style";
  var LOADER_ID = "zbar-usage-data-loader";
  var POLL_MS = 2000; /* 数据重载周期（与 Rust 导出周期一致） */
  var POLL_HIDDEN_MS = 10000; /* 页面隐藏时降频 */
  var FALLBACK_RENDER_MS = 15000; /* 低频兜底渲染周期（死锁/漏渲染自愈） */
  var STALE_MS = 90000; /* 活动轮枯萎判定阈值（V10）：目标节点文本连续
    无增长的时长，超时且该 umid 始终不在 index/runIndex（已完成被并入
    主轮的子代理轮，turns/runs 永无该行）→ 移除面板残留占位行并从活
    动轮目标中移除 */

  /* ---- 会话级实时统计条常量（V5，实机调参集中在此处） ---- */
  var SEL_SESSION_ID = "[data-session-id]"; /* 当前会话锚点（属性值为会话 id） */
  var ATTR_SESSION_ID = "data-session-id";
  var ATTR_SESSION_BAR = "data-zbar-usage-session"; /* 会话条标记（防重复挂载） */
  var ATTR_SESSION_BAR_FIXED = "data-zbar-usage-session-fixed"; /* 会话条兜底
    定位标记（V13）：region 缺失退回 fixed 时打上，样式表据此把 absolute
    切换为 fixed + bottom 兜底；迁回 region 时移除自动还原 */
  var SESSION_BAR_BOTTOM_PX = 96; /* 会话条 bottom（px）：仅 region 缺失的
    兜底路径使用（V13 正常定位为挂进容器的 absolute，见 renderSessionBar），
    实机可调 */
  /* V13 选择器记录（集中在常量区便于实机比对）：SEL_COMPOSER =
   * ".chat-composer-region" —— ZCode 输入区容器稳定语义类名（DevTools
   * 实测确认），renderSessionBar 把会话条幂等挂载进该容器（absolute
   * top:4px 住进 CSS 注入的顶部留白），不再做任何坐标测量 */
  var SEL_COMPOSER = ".chat-composer-region"; /* 输入区容器选择器，会话条
    挂载目标（V14 补回 V13 遗漏的定义，此前挂载块读未声明变量抛
    ReferenceError 被静默吞掉） */
  var COMPOSER_PAD_TOP_PX = 26; /* 输入区容器顶部留白（px）：ensureStyle 以
    !important 写死 26px（CSS 内无法引用 JS 变量），调整留白高度须同步
    修改两处——本常量（文档对照）与 ensureStyle 的 padding-top 规则 */
  var SESSION_BAR_Z = 30; /* 会话条 z-index：适度抬高，不遮挡弹层 */
  var VAR_SESSION_BAR = "--zbar-usage-session-bar"; /* 开关变量（variables.css 渲染 1/0） */
  var VAR_TURN_BAR = "--zbar-usage-turn-bar"; /* 每轮统计条开关变量（variables.css 渲染 1/0） */
  var TOKEN_CHARS = 3.5; /* 输出 token 估算系数：字符数/token（中英混合经验值，实机可调） */
  var DYN_TICK_MS = 200; /* 动态段刷新周期（每周期采样一次 textContent.length 差分） */
  var SPEED_WINDOW_MS = 1500; /* 速度滑动窗口（1~2 秒，平滑防抖动） */
  var SPEED_MIN_MS = 400; /* 参与速度计算的最小窗口（启动初期样本不足时速度记 0） */

  /* ---- 定位同目录 usage-data.js：由注入行自身 src 推导目录 ---- */
  var dataUrl = "";
  try {
    var tag =
      document.currentScript ||
      document.querySelector("script[data-zbar-usage]");
    if (tag && tag.src) {
      /* percent-encoded 路径不含裸 "?"，split 取基址安全（同 effects.js） */
      dataUrl = tag.src.split("?")[0].replace(/[^/]*$/, "") + "usage-data.js";
    }
  } catch (e) {
    dataUrl = "";
  }
  if (!dataUrl) return; /* 拿不到自身地址就无法定位数据文件，静默退出 */

  /* ---- 样式：内联创建样式表，不依赖 ZCode 类名；前景继承 + 半透明
   *      深浅主题均可读。V4 起字号/不透明度消费 variables.css 的
   *      --zbar-usage-font-size / --zbar-usage-opacity（兜底为 V3 写死值
   *      10px / .55；variables.css 由 effects.js 每秒热重载，var() 取值
   *      随变量自动重算，本样式表无需重建）。等待态低透明 = 正常态 ×
   *      0.636（默认 0.55×0.636≈0.35，与 V3 写死 .35 观感一致）。
   *      V5 追加会话累计条样式：pointer-events:none 防挡输入区交互、
   *      tabular-nums 防数字跳动宽度抖动，字号/不透明度与每轮条共用
   *      同一批变量。V13 会话条定位：absolute 挂进输入区容器（挂载见
   *      renderSessionBar）顶部留白（本函数注入的 region 规则，高度见
   *      COMPOSER_PAD_TOP_PX 注释，CSS 内写死 26px 须与常量同步）；
   *      region 缺失的兜底路径经 data-zbar-usage-session-fixed 属性切
   *      换为 fixed + bottom（SESSION_BAR_BOTTOM_PX，旧 V5 写法）---- */
  function ensureStyle() {
    if (document.getElementById(STYLE_ID)) return;
    var st = document.createElement("style");
    st.id = STYLE_ID;
    st.textContent =
      "[" + ATTR_ROW + "]{" +
      "display:block;margin:2px 0 0 1px;" +
      "font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "font-weight:400;line-height:1.5;" +
      "letter-spacing:.02em;color:inherit;opacity:var(--zbar-usage-opacity,.55);" +
      "font-variant-numeric:tabular-nums;" +
      "user-select:none;-webkit-user-select:none;" +
      "white-space:nowrap;overflow:hidden;text-overflow:ellipsis;}" +
      /* V8：等待态渲染分支已删除（活动轮判定不再需要 data-running），
       * 本样式保留作防御；当前无渲染路径产出 waiting 态 */
      "[" + ATTR_ROW + "][data-zbar-usage-row-state=waiting]{opacity:calc(var(--zbar-usage-opacity,.55)*.636)}" +
      /* V13：输入区容器顶部留白（高度写死 26px，与 COMPOSER_PAD_TOP_PX
       * 同步）——会话条 DOM 挂进容器、住进留白，输入框多行/窗口缩放随
       * 文档流自动跟随。relative 通常无副作用（region 自带 z-20，设计
       * 上已是定位元素）；若 ZCode 改用其它定位方案，本 !important 规
       * 则有覆盖风险，实机回访留意 */
      ".chat-composer-region{position:relative !important;padding-top:26px !important;}" +
      /* 会话累计条（V5）：不占消息布局。V13 定位改 absolute：住进输入
       * 区容器顶部留白（left + translateX 水平居中），不再 fixed/bottom */
      "[" + ATTR_SESSION_BAR + "]{" +
      "position:absolute;top:4px;left:50%;transform:translateX(-50%);" +
      "z-index:" + SESSION_BAR_Z + ";" +
      "font-family:ui-monospace,SFMono-Regular,Menlo,Consolas,monospace;" +
      "font-size:var(--zbar-usage-font-size,10px);" +
      "font-weight:400;line-height:1.5;" +
      "letter-spacing:.02em;color:inherit;opacity:var(--zbar-usage-opacity,.55);" +
      "font-variant-numeric:tabular-nums;" +
      "user-select:none;-webkit-user-select:none;" +
      "pointer-events:none;white-space:nowrap;}" +
      /* V13 兜底：region 缺失时条挂 body 并打上标记，退回旧 V5 行为
       * （fixed 贴窗底，bottom 见 SESSION_BAR_BOTTOM_PX） */
      "[" + ATTR_SESSION_BAR + "][" + ATTR_SESSION_BAR_FIXED + "]{" +
      "position:fixed;top:auto;bottom:" + SESSION_BAR_BOTTOM_PX + "px;}";
    (document.head || document.documentElement).appendChild(st);
  }

  /* ---- 数据索引：umid（用户消息 id，与 DOM data-turn-id 同值）→ 轮对象 ---- */
  var index = {};
  /* 进行中轮索引（V6）：umid → run 行（model_usage 已落库、turn_usage
   * 尚未写入的轮实时聚合）。完成轮由 index 接管（优先级更高），2 秒
   * 竞态窗口内同 umid 两边并存时按完成数据渲染 */
  var runIndex = {};
  var lastTs = null; /* 上次加载的数据 ts（未变则本轮零成本跳过） */
  /* 最近一次数据的 turns / runs 原始数组：index/runIndex 丢弃了 umid
   * 为 null 的行，而会话累计必须覆盖会话内全部行（含无法与 DOM 匹配
   * 的子代理轮——经 psess 并入），故聚合时遍历原始数组而非索引 */
  var lastTurns = [];
  var lastRuns = [];

  function rebuildIndex(data) {
    index = {};
    runIndex = {};
    lastTurns = (data && data.turns) || [];
    lastRuns = (data && data.runs) || [];
    if (data && data.turns && data.turns.length) {
      for (var i = 0; i < data.turns.length; i++) {
        var t = data.turns[i];
        /* umid 为 null 的轮（老版本库缺列/无关联消息）无法与 DOM 匹配 */
        if (t && t.umid) index[t.umid] = t;
      }
    }
    if (data && data.runs && data.runs.length) {
      for (var j = 0; j < data.runs.length; j++) {
        var r = data.runs[j];
        /* 旧导出文件无 runs 数组：lastRuns 兜底为空数组，零影响 */
        if (r && r.umid) runIndex[r.umid] = r;
      }
    }
  }

  /* ---- 数据轮询：script 标签方式重载（onload/onerror 均静默） ---- */
  var loading = false;

  function loadData() {
    if (loading || !dataUrl) return;
    loading = true;
    var old = document.getElementById(LOADER_ID);
    if (old && old.parentNode) old.parentNode.removeChild(old);
    var s = document.createElement("script");
    s.id = LOADER_ID;
    s.onload = function () {
      loading = false;
      try {
        var data = window.__ZBAR_USAGE__;
        /* 版本不符视为无效数据，走静默路径（保留上次数据） */
        if (!data || data.v !== 2) return;
        /* ts 未变：Rust 侧仅在内容变化时重写文件，常态重载零成本跳过 */
        if (data.ts === lastTs) return;
        lastTs = data.ts;
        rebuildIndex(data);
        scheduleRender();
      } catch (e) {
        /* 单轮失败静默 */
      }
    };
    s.onerror = function () {
      loading = false; /* 加载失败保留上次数据 */
    };
    s.src = dataUrl + "?t=" + Date.now();
    (document.head || document.documentElement).appendChild(s);
  }

  function pollLoop() {
    try {
      loadData();
    } catch (e) {
      /* 静默 */
    }
    setTimeout(pollLoop, document.hidden ? POLL_HIDDEN_MS : POLL_MS);
  }

  /* ---- 格式化（速度口径见文件头说明） ---- */
  function trimTail(s) {
    return s.replace(/\.0$/, "");
  }

  /* V10：token 值等宽补位——恒定 5 字符（"  998"/" 1.2k"/"10.5M"，超过
   * 999.9M 自然溢出），等宽字体 + tabular-nums 下字段占位恒定 */
  function fmtTokens(n) {
    n = Number(n) || 0;
    var sign = n < 0 ? "-" : "";
    n = Math.abs(n);
    var s;
    if (n < 1000) s = String(Math.round(n));
    else if (n < 1000000) s = trimTail((n / 1000).toFixed(1)) + "k";
    else s = trimTail((n / 1000000).toFixed(1)) + "M";
    return (sign + s).padStart(5, " ");
  }

  function fmtSeconds(ms) {
    return (Math.round(ms / 100) / 10).toFixed(1) + "s";
  }

  /* V10：速度字段等宽补位——恒定 4 字符（" 0.0"~"99.9"，≥100 自然溢
   * 出）；null（dur 缺失老库）显示 4 字符占位 "  - " */
  function padSpeed(s) {
    return s == null ? "  - " : s.toFixed(1).padStart(4, " ");
  }

  /* 生成窗口：与 db.rs gen_window_expr 同口径（保守取小值） */
  function genMs(t) {
    var d = t.dur;
    if (d == null || d <= 0) return 0;
    var ttft = t.ttft;
    if (ttft != null && ttft >= 0 && ttft <= d) {
      if (ttft * 10 >= d * 9) return ttft; /* 整块下发：gen = ttft */
      return Math.max(1, d - ttft);
    }
    return d;
  }

  /* ---- 统一行格式（V10）：三态共用同一固定结构，只更新字段数值 ----
   * "↑ <in> ↓ <out> ⟲ <cr> · × <req> · <speed> t/s · TTFT <ttft>"
   * 各字段等宽补位（等宽字体 + tabular-nums 下整行宽度恒定）：
   * - token 字段经 fmtTokens 恒定 5 字符；
   * - req padStart(3)；
   * - 速度经 padSpeed 恒定 4 字符（null = dur 缺失老库，显示占位）；
   * - TTFT padStart(4)（"x.xs" 恒 4 字符），null（进行中未定/老库缺
   *   失）显示 "–" 占位；
   * - ↓ 前固定 1 字符估算前缀位：进行中（live/启动窗口）"~"、完成
   *   空格，宽度恒定。
   * 三态入参：启动窗口数字位 0 / 估算值（↓ 为估算输出）；live 真实
   * 聚合（含 sub）+ 估算叠加；完成最终值 + TTFT 数值。 */
  function barLineOf(v) {
    return (
      "↑ " + fmtTokens(v.inp) +
      " ↓ " + (v.est ? "~" : " ") + fmtTokens(v.out) +
      " ⟲ " + fmtTokens(v.cr) +
      " · × " + String(v.req).padStart(3, " ") +
      " · " + padSpeed(v.speed) + " t/s" +
      " · TTFT " + (v.ttft == null ? "–" : v.ttft).padStart(4, " ")
    );
  }

  function lineOf(t) {
    var subOut = t.sub ? t.sub.out || 0 : 0;
    var g = genMs(t);
    return barLineOf({
      inp: Math.max(0, (t["in"] || 0) - (t.cr || 0)),
      out: t.out || 0,
      cr: t.cr || 0,
      req: t.req || 0,
      /* dur 缺失（g = 0）时速度位显示占位（V10 固定结构，原省略删除） */
      speed: g >= 1 ? ((t.out || 0) + subOut) * 1000 / g : null,
      ttft: t.ttft != null && t.ttft >= 0 ? fmtSeconds(t.ttft) : null,
      est: false
    });
  }

  /* ---- 进行中轮行（V6）：run 真实聚合 + 当前流式输出估算叠加到 ↓ +
   *      估算速度段。V10 统一固定结构：结构恒定只更新数值（速度 0 时
   *      显示 0.0，TTFT 位 "–" 表达进行中，原行尾 … 标记删除）。V9：数
   *      字段合计数据侧并入的子代理实时聚合（r.sub），estTok 仍只叠加
   *      本会话活动轮节点；非估算目标的并行 live 节点 estTok/estSpeed
   *      为 0，同样渲染完整固定结构 ---- */
  function liveLineOf(r, estTok, estSpeed) {
    var s = r.sub;
    var inp = (r["in"] || 0) + (s ? s["in"] || 0 : 0);
    var out = (r.out || 0) + (s ? s.out || 0 : 0);
    var cr = (r.cr || 0) + (s ? s.cr || 0 : 0);
    var req = (r.req || 0) + (s ? s.req || 0 : 0);
    return barLineOf({
      inp: Math.max(0, inp - cr),
      out: out + estTok,
      cr: cr,
      req: req,
      speed: estSpeed > 0 ? estSpeed : 0,
      ttft: null,
      est: true
    });
  }

  /* ---- 渲染：按 umid（= data-turn-id 值）匹配；同一轮被虚拟列表拆成
   *      多个单元节点时，统计条只渲染在 DOM 顺序最后一个节点（回复结束
   *      处），其余节点旧行移除；幂等（已存在则更新内容） ---- */
  var rendered = new Map(); /* umid → 统计条元素（虚拟列表回收时清） */

  function ensureRow(node, turnId) {
    var row = node.querySelector(":scope > [" + ATTR_ROW + "]");
    if (!row) {
      row = document.createElement("div");
      row.setAttribute(ATTR_ROW, "");
      node.appendChild(row); /* section 内末尾 append */
    }
    rendered.set(turnId, row);
    return row;
  }

  function removeRow(node, turnId) {
    var row = node.querySelector(":scope > [" + ATTR_ROW + "]");
    if (row && row.parentNode) row.parentNode.removeChild(row);
    rendered.delete(turnId);
  }

  function renderOne(node, turnId, isLast, activeMap) {
    var t = index[turnId];
    if (!t) {
      /* V6：进行中 run 命中——真实聚合 + 流式估算实时条（只在最后一个
       * 节点渲染）。2 秒竞态窗口内 index 与 runIndex 并存时走上方完成
       * 分支（完成数据优先），此处天然只在"确实未完成"时到达。
       * V9：数字段含数据侧并入的子代理聚合（见 liveLineOf），估算只
       * 叠加本会话活动轮节点（estFor）。V16 起不再有悬浮 title */
      var r = runIndex[turnId];
      if (r) {
        if (!isLast) {
          removeRow(node, turnId);
          return;
        }
        var live = ensureRow(node, turnId);
        var est = estFor(node);
        var text = liveLineOf(r, est.tok, est.speed);
        if (live.textContent !== text) live.textContent = text;
        live.setAttribute("data-zbar-usage-row-state", "live");
        return;
      }
      /* V8：启动窗口活动轮（DOM 驱动判定，见 findLiveNodes）——消息发
       * 出节点即在 DOM，无需等待任何数据库数据，立即渲染。V10：统一
       * 固定结构（数字位 0 / 估算值，↓ 为估算输出带 ~ 前缀，TTFT 位
       * "–"；原"逐段省略"极简行删除），首笔请求完成（runIndex 命中）
       * 后切 live 态、turn_usage 落库后切完成态，结构全程不变。run 数
       * 据未达不再是渲染阻塞项；V9 多容器：节点为其所属会话的活动轮
       * （activeMap 按会话 id 索引）即渲染。V10：枯萎目标维持移除 */
      if (activeMap.get(sessOf(node)) === node) {
        var liveSess = sessOf(node);
        if (staleHolds(liveSess, node, turnId)) {
          removeRow(node, turnId);
          return;
        }
        var estStart = estFor(node);
        var startRow = ensureRow(node, turnId);
        var startText = barLineOf({
          inp: 0,
          out: estStart.tok,
          cr: 0,
          req: 0,
          speed: estStart.speed > 0 ? estStart.speed : 0,
          ttft: null,
          est: true
        });
        if (startRow.textContent !== startText) startRow.textContent = startText;
        startRow.setAttribute("data-zbar-usage-row-state", "live");
        return;
      }
      /* 未命中且非活动轮：不渲染（避免脏数据），清掉可能的残留条 */
      removeRow(node, turnId);
      return;
    }
    if (!isLast) {
      /* 数据命中但本节点不是该轮最后一个节点：清掉本节点上的行
       *（节点集合变化导致"最后一个"迁移时，旧节点上的旧行在此清理） */
      removeRow(node, turnId);
      return;
    }
    var row = ensureRow(node, turnId);
    var text = lineOf(t);
    if (row.textContent !== text) row.textContent = text;
    row.setAttribute("data-zbar-usage-row-state", "data");
  }

  /* ---- 会话级实时统计条（V5 引入，V6 实时化，V8 启动窗口化，V9 子代
   *      理实时化）：fixed 悬浮于对话输入框上方，Σ = 完成轮合计（sess
   *      过滤 turns 原始数组，已含并入的子代理部分）+ 进行中 run 合计
   *      （本会话行含并入的子代理 sub + 未打 m 标记的子代理行按 psess
   *      命中，见 sessionRunTotals），生成期间数字持续跳动；V8 起估算
   *      输出不再叠加进 Σ ↓ 真实数字，改在动态段显示（V15 起格式为
   *      "X.X t/s · ≈X"：速度段去 ⋯ 前缀、估算段改 ≈ 前缀），且动态段
   *      从活动轮判定即启动（含 runs 未达的启动窗口轮）。独立管线：renderAll 尾部调用（活动轮
   *      判定由 renderAll 统一计算传入，V9 起为多容器 Map）+ 动态定时
   *      器统一采样自驱动，任何 DOM/数据异常都 try 静默，不影响每轮条。
   *      开关经 --zbar-usage-session-bar（variables.css 渲染 1/0，热重
   *      载生效）读取，变量缺失视为开启（默认 true，与 ThemeParams 默
   *      认值一致） ---- */
  var sessionBar = null; /* 已挂载的会话条元素（跨渲染复用，幂等更新） */
  var mountWarned = false; /* 挂载异常一次性告警标志（V14）：此前挂载
    catch 静默吞掉异常（含 V13 的 ReferenceError），兜底态无法被发现 */
  /* 流式估算状态（V9 多目标）：会话 id → { node, startChars, samples }，
   * 单一定时器统一采样驱动，各会话活动轮独立差分估算互不干扰；目标的
   * 建立/换轮重建/清理统一由 syncDyn 依活动轮判定结果处理 */
  var dyn = { timer: 0, targets: new Map() };
  var EST_ZERO = { tok: 0, speed: 0 }; /* 非估算节点的空估算（共享只读） */
  /* 枯萎目标记录（V10）：sess → { id: umid, chars: 枯萎时文本长度 }。
   * 目标文本连续 STALE_MS 无增长且 umid 始终不在 index/runIndex 时写入
   * （已完成被并入主轮的子代理轮，turns/runs 永无该行）：同会话同
   * umid 的活动轮不再重建估算目标、不再渲染启动占位行；目标文本变化
   * （恢复输出/虚拟列表重挂内容变化）时消费端 staleHolds 失效记录，
   * 重新评估（无害） */
  var stale = new Map();

  /* 枯萎记录是否仍对该节点生效：umid 相同且文本长度无变化视为仍枯萎；
   * 文本已变化则删除记录并放行（重新评估） */
  function staleHolds(sess, node, turnId) {
    var st = stale.get(sess);
    if (!st || st.id !== turnId) return false;
    var ch = 0;
    try {
      ch = (node.textContent || "").length;
    } catch (e) {
      ch = 0;
    }
    if (ch !== st.chars) {
      stale.delete(sess);
      return false;
    }
    return true;
  }

  function sessionBarEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_SESSION_BAR
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* V19：每轮统计条开关（镜像 sessionBarEnabled），renderAll 第二遍渲染
   * 前统一读取（单次 getComputedStyle，避免逐节点重复读） */
  function turnBarEnabled() {
    try {
      var v = (
        getComputedStyle(document.documentElement).getPropertyValue(
          VAR_TURN_BAR
        ) || ""
      ).trim();
      return v !== "0";
    } catch (e) {
      return true; /* 读不到变量（旧 variables.css）按默认开启 */
    }
  }

  /* V17：元素当前是否可见（保活面板隐藏时 rect 归零或 display:none）*/
  function visibleEl(el) {
    try {
      var r = el.getBoundingClientRect();
      var s = window.getComputedStyle(el);
      return r.width > 0 && r.height > 0 &&
        s.display !== "none" && s.visibility !== "hidden";
    } catch (e) {
      return false;
    }
  }

  /* 当前会话 id（V17 重写）：旧实现 querySelector 首中即取，多会话保
   * 活时首中的是任务切换后仍挂载在 DOM 的旧会话容器（通常排在前面），
   * 导致新建任务后条继续渲染旧会话累计、永不消失。改为遍历候选容器
   * 按两级优先取值：① 包含 document.activeElement 的容器（用户正在
   * 输入的会话，含焦点时通常即当前任务）；② 第一个可见容器
   * （visibleEl，保活面板隐藏时 rect 归零或 display:none）。锚点内优
   * 先——子代理详情面板容器与主对话同 document，须先限定主对话锚点
   * 范围；锚点内无候选再退化为全文档。都不满足返回空串（renderSessionBar
   * 既有逻辑随之 removeBar） */
  function currentSessionId(anchor) {
    try {
      var scopes = anchor ? [anchor, document] : [document];
      for (var s = 0; s < scopes.length; s++) {
        var list = scopes[s].querySelectorAll(SEL_SESSION_ID);
        var firstVisible = null;
        for (var i = 0; i < list.length; i++) {
          var el = list[i];
          /* ① 焦点所在容器（用户正在输入的会话）优先即取 */
          try {
            if (document.activeElement &&
              el.contains(document.activeElement)) {
              return el.getAttribute(ATTR_SESSION_ID) || "";
            }
          } catch (e2) {}
          /* ② 记录第一个可见容器，扫完无焦点命中再取 */
          if (firstVisible === null && visibleEl(el)) {
            firstVisible = el.getAttribute(ATTR_SESSION_ID) || "";
          }
        }
        if (firstVisible !== null) return firstVisible;
      }
      return "";
    } catch (e) {
      return "";
    }
  }

  /* 会话累计（完成轮真实值）：按 sess 过滤 turns 原始数组聚合。该会话
   * 无任何完成轮返回 null——V6 起不再据此直接放弃渲染（V5 根因 a)：
   * 新会话首轮生成期间 totals 恒为 null，动态段从未启动），改由
   * renderSessionBar 以 run 合计兜底 */
  function sessionTotals(sessId) {
    var tin = 0,
      tout = 0,
      tcr = 0,
      treq = 0,
      found = false;
    for (var i = 0; i < lastTurns.length; i++) {
      var t = lastTurns[i];
      if (!t || t.sess !== sessId) continue;
      found = true;
      /* 与每轮条同口径（保守取小值）逐轮累计 */
      tin += Math.max(0, (t["in"] || 0) - (t.cr || 0));
      tout += t.out || 0;
      tcr += t.cr || 0;
      treq += t.req || 0;
    }
    return found ? { tin: tin, tout: tout, tcr: tcr, treq: treq } : null;
  }

  /* 进行中 run 合计（V6 引入，V9 口径修正）：本会话行（sess 命中，含
   * 并入的子代理 sub）+ 未打 m 标记的子代理行（psess 命中）。V9 修正：
   * a) 跳过 m:1 的子代理行——其数值已并入父会话主轮行 sub，随主轮行
   *    一并计入（原 psess 裸命中在子代理行与主轮行并存时双计）；
   * b) 主会话行携带 sub（并行子代理 runs + 游离子代理完成轮）时按行
   *    数值同口径合计计入——子代理消耗实时反映在会话累计条上；
   * c) 父会话暂无主轮行（主轮首笔请求未完成）的子代理行不带 m，仍按
   *    psess 在此并入，主轮行出现后自动切换口径，无缝衔接。
   * 无命中返回 null */
  function sessionRunTotals(sessId) {
    var tin = 0,
      tout = 0,
      tcr = 0,
      treq = 0,
      found = false;
    for (var i = 0; i < lastRuns.length; i++) {
      var r = lastRuns[i];
      if (!r || r.m) continue;
      if (r.sess !== sessId && r.psess !== sessId) continue;
      found = true;
      tin += Math.max(0, (r["in"] || 0) - (r.cr || 0));
      tout += r.out || 0;
      tcr += r.cr || 0;
      treq += r.req || 0;
      if (r.sub) {
        tin += Math.max(0, (r.sub["in"] || 0) - (r.sub.cr || 0));
        tout += r.sub.out || 0;
        tcr += r.sub.cr || 0;
        treq += r.sub.req || 0;
      }
    }
    return found ? { tin: tin, tout: tout, tcr: tcr, treq: treq } : null;
  }

  /* 活动轮判定（V8 DOM 驱动，V9 多容器化）：遍历 document 上所有
   * [data-session-id] 容器（主对话 + 子代理详情面板同 document 共存，
   * 各有独立容器，并行多个子代理各有面板），每容器取 DOM 顺序最后一个
   * data-turn-id（umid）节点，umid 不在 index（完成轮）即为该会话的
   * 活动轮。修复 V8 锚点扫描扫不到子代理面板的缺陷（面板在
   * workspace-main 锚点之外）。判定不依赖 runs 数据：既不在 index 也
   * 不在 runIndex = 启动窗口活动轮；在 runIndex = runs 阶段活动轮（同
   * 一节点继续估算叠加），V8 口径不变。返回 Map：会话 id → 活动轮节点
   * （每轮条启动窗口分支、live 估算叠加与会话条动态段共用同一次判定）。
   * 嵌套容器防御：只认属主为本容器的节点（最近 [data-session-id] 祖先
   * 必须是本容器），防把子容器（子代理面板）的轮算进父容器。零容器时
   * 退化为 document 末节点（键取空串，与 sessOf 无容器返回值一致） */
  function findLiveNodes() {
    var map = new Map();
    try {
      var containers = document.querySelectorAll(SEL_SESSION_ID);
      for (var i = 0; i < containers.length; i++) {
        var sess = (containers[i].getAttribute(ATTR_SESSION_ID) || "");
        if (!sess || map.has(sess)) continue;
        var scoped = containers[i].querySelectorAll(SEL_TURN);
        var last = null;
        for (var j = scoped.length - 1; j >= 0; j--) {
          if (scoped[j].closest(SEL_SESSION_ID) === containers[i]) {
            last = scoped[j];
            break;
          }
        }
        if (!last) continue;
        var id = last.getAttribute(ATTR_TURN_ID);
        if (!id || index[id]) continue; /* 已落库 = 已完成 */
        map.set(sess, last);
      }
      if (!containers.length) {
        var nodes = document.querySelectorAll(SEL_TURN);
        if (nodes.length) {
          var lastNode = nodes[nodes.length - 1];
          var lastId = lastNode.getAttribute(ATTR_TURN_ID);
          if (lastId && !index[lastId]) map.set("", lastNode);
        }
      }
    } catch (e) {
      /* 静默，返回已收集部分 */
    }
    return map;
  }

  /* V13：ensureBar 只负责单例创建与断连自愈，挂载位置（region 正常路
   * 径 / body 兜底路径）由 renderSessionBar 依 region 是否存在决定 */
  function ensureBar() {
    if (sessionBar && sessionBar.isConnected) {
      return sessionBar;
    }
    removeBar(); /* 断连后重建（页面清掉注入层/region 被重建时自愈） */
    var bar = document.createElement("div");
    bar.setAttribute(ATTR_SESSION_BAR, "");
    sessionBar = bar;
    return bar;
  }

  function removeBar() {
    if (sessionBar && sessionBar.parentNode) {
      sessionBar.parentNode.removeChild(sessionBar);
    }
    sessionBar = null;
  }

  /* 估算定时器：仅存在估算目标时运行，全空必清（防泄漏） */
  function stopDyn() {
    if (dyn.timer) {
      clearInterval(dyn.timer);
      dyn.timer = 0;
    }
    dyn.targets.clear();
  }

  /* 估算目标管理（V9，renderAll 每次调用）：activeMap 为最新活动轮判
   * 定（sess → 节点）。目标节点变化（换轮）或消失（轮完成/虚拟列表回
   * 收）即重建/移除，新活动轮出现即建立目标——起始基准 = 建立时刻的
   * 首采样，检测延迟内的少量输出不计（保守取小）。有目标保证定时器运
   * 行，无目标停止。轮完成时 umid 进入 index → findLiveNodes 不再返回
   * 该会话 → 目标移除，Σ 与每轮条在 2 秒内经数据轮询切回 turn_usage
   * 真实值，直接切换不平滑过渡 */
  function syncDyn(activeMap) {
    try {
      var changed = false;
      dyn.targets.forEach(function (tgt, sess) {
        if (!tgt.node || !tgt.node.isConnected || activeMap.get(sess) !== tgt.node) {
          dyn.targets.delete(sess);
          changed = true;
        }
      });
      activeMap.forEach(function (node, sess) {
        if (!dyn.targets.has(sess)) {
          var chars = 0;
          try {
            chars = (node.textContent || "").length;
          } catch (e2) {
            chars = 0;
          }
          /* V10：lastChars/lastGrow 供枯萎判定追踪文本增长 */
          dyn.targets.set(sess, {
            node: node,
            startChars: chars,
            samples: [],
            lastChars: chars,
            lastGrow: Date.now()
          });
          changed = true;
        }
      });
      if (!changed) return;
      if (dyn.targets.size) {
        if (!dyn.timer) dyn.timer = setInterval(dynTick, DYN_TICK_MS);
      } else {
        stopDyn();
      }
    } catch (e) {
      /* 静默 */
    }
  }

  /* 统一采样驱动（每 DYN_TICK_MS）：全部目标各采样一次 textContent
   * 长度差分，滑动窗口只保留窗口内样本（窗口恒定，样本数有界）；节点
   * 被回收的目标在此清除。200ms 周期本身就是节流；全量重渲同时覆盖每
   * 轮条（估算 ↓ 与速度段）与各会话条（Σ 跳动）。
   * V5 曾在此检查 data-running 属性——实机不可靠，V6 已移除，V9 沿用。
   * V10 枯萎判定：目标文本连续 STALE_MS 无增长且该 umid 始终不在
   * index/runIndex = 已完成被并入主轮的子代理轮（turns/runs 永无该
   * 行，面板节点永久残留启动占位行）——移除该行并从活动轮目标中移除
   * （记录进 stale，文本再变化时消费端 staleHolds 重新评估，无害） */
  function dynTick() {
    try {
      var now = Date.now();
      dyn.targets.forEach(function (tgt, sess) {
        if (!tgt.node || !tgt.node.isConnected) {
          dyn.targets.delete(sess);
          return;
        }
        var c = (tgt.node.textContent || "").length;
        tgt.samples.push({ t: now, c: c });
        while (tgt.samples.length > 2 && now - tgt.samples[0].t > SPEED_WINDOW_MS) {
          tgt.samples.shift();
        }
        if (c > tgt.lastChars) {
          tgt.lastChars = c;
          tgt.lastGrow = now;
        } else if (now - tgt.lastGrow > STALE_MS) {
          var tid = tgt.node.getAttribute(ATTR_TURN_ID);
          if (tid && !index[tid] && !runIndex[tid]) {
            stale.set(sess, { id: tid, chars: c });
            removeRow(tgt.node, tid);
            dyn.targets.delete(sess);
          }
        }
      });
      if (!dyn.targets.size) stopDyn();
      renderAll();
    } catch (e) {
      /* 单轮采样失败静默，下个周期再试 */
    }
  }

  /* 当前流式估算（V6 取代 V5 的 dynSegmentText 拼串；V9 按会话取目标）：
   * tok = 目标建立时刻起的字符增量 ÷ TOKEN_CHARS；speed = 滑动窗口差分
   * 求速（样本不足 SPEED_MIN_MS 时记 0，消费端省略速度段）。消费口径：
   * 各会话 live 行（runIndex 命中）叠加到 ↓（~ 前缀），启动窗口行与
   * 会话条动态段独立显示（会话条 V15 起为 ≈est，不并入真实数字）；
   * 非估算运行（无定时器/无目标）返回 EST_ZERO */
  function dynEstimate(sess) {
    if (!dyn.timer) return EST_ZERO;
    var tgt = dyn.targets.get(sess);
    if (!tgt) return EST_ZERO;
    try {
      var node = tgt.node;
      var chars = (node.textContent || "").length;
      var tok = Math.max(0, (chars - tgt.startChars) / TOKEN_CHARS);
      var speed = 0;
      if (tgt.samples.length) {
        var old = tgt.samples[0];
        var dt = Date.now() - old.t;
        if (dt >= SPEED_MIN_MS && chars > old.c) {
          speed = ((chars - old.c) / TOKEN_CHARS) * 1000 / dt;
        }
      }
      return { tok: tok, speed: speed };
    } catch (e) {
      return EST_ZERO;
    }
  }

  /* 节点所属会话 id（V9）：向上取最近 [data-session-id] 容器属性值；
   * 无容器返回空串（与 findLiveNodes 的零容器兜底键一致） */
  function sessOf(node) {
    try {
      var c = node.closest(SEL_SESSION_ID);
      return (c && c.getAttribute(ATTR_SESSION_ID)) || "";
    } catch (e) {
      return "";
    }
  }

  /* 节点的实时估算（V9）：仅当该节点是其所属会话的当前估算目标时返回
   * 估算，否则空估算（非活动/非目标的并行 live 节点显示纯真实聚合） */
  function estFor(node) {
    var sess = sessOf(node);
    var tgt = dyn.targets.get(sess);
    return tgt && tgt.node === node ? dynEstimate(sess) : EST_ZERO;
  }

  /* V17：会话条挂载点选择。多会话保活时 document 首个
   * .chat-composer-region 可能属于隐藏的旧会话容器（首中即用会把条挂
   * 错位置），改为两级选择：① 可见且属于当前会话容器（closest 命中
   * 会话容器属性值 === sessId，分栏视图下跟随当前会话）；② 第一个可
   * 见的。都无返回 null，走 renderSessionBar 既有 body 兜底分支 */
  function pickComposerRegion(sessId) {
    try {
      var list = document.querySelectorAll(SEL_COMPOSER);
      var firstVisible = null;
      for (var i = 0; i < list.length; i++) {
        var el = list[i];
        if (!visibleEl(el)) continue;
        if (firstVisible === null) firstVisible = el;
        var owner = el.closest ? el.closest(SEL_SESSION_ID) : null;
        if (sessId && owner &&
          (owner.getAttribute(ATTR_SESSION_ID) || "") === sessId) {
          return el;
        }
      }
      return firstVisible;
    } catch (e) {
      return null;
    }
  }

  function renderSessionBar(anchor, activeMap) {
    if (!sessionBarEnabled()) {
      /* V9：估算目标已独立管理（syncDyn），关闭会话条不再连带停估算
       * ——live 每轮条仍需估算叠加 */
      removeBar();
      return;
    }
    var sessId = currentSessionId(anchor);
    /* 找不到锚点/会话 id：不渲染（静默降级）。锚点定位天然只在主窗口
     * 出会话条（子代理面板在锚点外，无输入区不渲染会话条） */
    if (!anchor || !sessId) {
      removeBar();
      return;
    }
    var totals = sessionTotals(sessId);
    var runTotals = sessionRunTotals(sessId);
    /* V8：完成合计、run 合计均空且无活动轮（DOM 驱动判定）才放弃——
     * 启动窗口活动轮同样支撑会话条即时显示（新会话首轮发消息即出现，
     * 修复 V6/V7 首笔请求完成前会话条全空白的启动窗口）。draft 空会话
     * 仍不渲染。V9：活动轮按本会话 id 从 activeMap 取 */
    var active = activeMap.has(sessId);
    if (!totals && !runTotals && !active) {
      removeBar();
      return;
    }
    var est = dyn.timer ? dynEstimate(sessId) : EST_ZERO;
    /* Σ 真实部分 = 完成轮合计 + 进行中 run 合计。V8：流式估算输出不再
     * 叠加进 ↓ 真实数字（避免估算污染累计），改入下方动态段 ≈est。
     * 轮完成切换时刻：run 合计消失、完成合计接管——最后一笔进行中请求
     * 完成后才计入 turn_usage，切换瞬间数字可能小幅修正，属预期误差 */
    var tin = (totals ? totals.tin : 0) + (runTotals ? runTotals.tin : 0);
    var tout =
      (totals ? totals.tout : 0) + (runTotals ? runTotals.tout : 0);
    var tcr = (totals ? totals.tcr : 0) + (runTotals ? runTotals.tcr : 0);
    var treq = (totals ? totals.treq : 0) + (runTotals ? runTotals.treq : 0);
    /* V10：Σ 数字段等宽补位（token 经 fmtTokens 恒定 5 字符、req
     * padStart(3)），行宽不随数值位数跳动。V15：Σ 段为会话总 Token
     * （tsum = 输入+输出+缓存读之和，真实数据不含估算），明细段依次
     * 为 ↑ 输入 / ↓ 输出 / ⟲ 缓存读 / × 请求数 */
    var tsum = tin + tout + tcr;
    var parts = [
      "Σ " + fmtTokens(tsum),
      "↑ " + fmtTokens(tin),
      "↓ " + fmtTokens(tout),
      "⟲ " + fmtTokens(tcr),
      "× " + String(treq).padStart(3, " ")
    ];
    /* V10 动态段固定：两段永远显示（idle 无活动轮时速度 0.0、估算 0），
     * 不再按有无值省略，Σ 行整体宽度恒定。启动窗口轮（runs 未达）与
     * runs 阶段统一走此段。V15：速度段去掉 ⋯ 前缀（与每轮条一致）；
     * 估算段改 ≈ 前缀（生成中未落库的输出估算，不计入累计） */
    parts.push(padSpeed(est.speed || 0) + " t/s");
    parts.push("≈" + fmtTokens(est.tok));
    var text = parts.join(" · ");
    var bar = ensureBar();
    /* V13：挂载进输入区容器（幂等迁移）——条 absolute 住进 CSS 注入的
     * 26px 顶部留白，零坐标测量、随文档流自适应。V17：挂载点经
     * pickComposerRegion 选择（可见优先并跟随当前会话容器，多会话保
     * 活时不再首中隐藏旧会话的输入区容器）。region 缺失（选择器失效/
     * 结构变更）退回旧路径：挂 body、打兜底标记切 fixed +
     * SESSION_BAR_BOTTOM_PX（迁回 region 时移除标记自动还原） */
    try {
      var region = pickComposerRegion(sessId);
      if (region) {
        if (bar.parentElement !== region) region.appendChild(bar);
        if (bar.hasAttribute(ATTR_SESSION_BAR_FIXED)) {
          bar.removeAttribute(ATTR_SESSION_BAR_FIXED);
        }
      } else {
        if (bar.parentElement !== document.body) {
          document.body.appendChild(bar);
        }
        bar.setAttribute(ATTR_SESSION_BAR_FIXED, "");
      }
    } catch (e) {
      /* 挂载异常：一次性告警（V14 前此处静默吞掉了 ReferenceError，
       * 导致兜底态无法被发现），之后每周期静默重试 */
      if (!mountWarned) {
        mountWarned = true;
        try { console.warn("[ZBar] usage session bar mount error:", e); } catch (e2) {}
      }
    }
    if (bar.textContent !== text) bar.textContent = text;
  }

  function renderAll() {
    /* V9：document 级扫描——子代理详情面板与主对话同 document 且在
     * workspace-main 锚点之外，每轮条扫描不再限定锚点（35 节点量级，
     * 性能无虞）；锚点仅保留给会话条定位 */
    var anchor = document.querySelector(SEL_PANE_ANCHOR);
    var nodes = document.querySelectorAll(SEL_TURN);
    if (!nodes.length) {
      stopDyn(); /* 会话清空即停估算（防幽灵目标空转） */
      /* V18：空会话（新建任务/清空对话）同样要推进会话条——此前此处
       * 早退导致 renderSessionBar 永不执行，条停留在上一个会话的
       * 累计值永不消失（实机复现：新任务容器 data-session-id="draft"
       * 可见且含焦点，V17 的会话判定本身正确，缺的就是这一步） */
      removeBar();
      return;
    }
    /* 第一遍：按 DOM 顺序确定每个 umid 的最后一个节点（querySelectorAll
     * 保证 DOM 顺序，后者覆盖前者即"最后一个"；同一 umid 可能命中多个
     * 虚拟列表单元节点） */
    var lastOf = {};
    var seen = {};
    for (var i = 0; i < nodes.length; i++) {
      var id = nodes[i].getAttribute(ATTR_TURN_ID);
      if (!id) continue;
      seen[id] = true;
      lastOf[id] = nodes[i];
    }
    /* V9：多容器活动轮判定（每会话一个活动轮，Map：sess → 节点；每轮
     * 条启动窗口分支、live 估算叠加与会话条动态段共用同一次判定结果，
     * 见 findLiveNodes） */
    var activeMap = findLiveNodes();
    /* 估算目标管理（V9 多目标）：依最新判定建立/重建/清理各会话目标 */
    syncDyn(activeMap);
    /* 第二遍：逐节点渲染（仅"最后一个"节点出内容，其余节点清旧行）。
     * V19：每轮统计条开关——循环前读一次 --zbar-usage-turn-bar（变量
     * 缺失视为开启，兼容旧 variables.css），关闭时对全部轮节点
     * removeRow 并跳过 renderOne（幂等清掉已渲染行，开启后自动恢复）；
     * syncDyn 估算目标与会话条管线不受影响 */
    var turnBarOn = turnBarEnabled();
    for (var i = 0; i < nodes.length; i++) {
      var id = nodes[i].getAttribute(ATTR_TURN_ID);
      if (!id) continue;
      if (!turnBarOn) {
        removeRow(nodes[i], id);
        continue;
      }
      renderOne(nodes[i], id, lastOf[id] === nodes[i], activeMap);
    }
    /* 虚拟列表回收：节点从 DOM 消失（连带统计条断连）即清缓存不残留 */
    rendered.forEach(function (rowEl, id) {
      if (!seen[id] || !rowEl.isConnected) rendered.delete(id);
    });
    /* 会话级实时统计条（V5）：独立管线，失败不影响每轮条；
     * 动态运行期另由 DYN_TICK_MS 定时器统一采样自驱动刷新 */
    try {
      renderSessionBar(anchor, activeMap);
    } catch (e) {
      /* 会话条失败静默，下个变更/兜底周期重试 */
    }
  }

  var scheduled = false;

  function scheduleRender() {
    if (scheduled) return;
    scheduled = true;
    var raf = window.requestAnimationFrame || function (f) { setTimeout(f, 50); };
    raf(function () {
      try {
        renderAll();
      } catch (e) {
        /* 单轮失败静默，下次变更重试 */
      } finally {
        /* 复位必须在 finally：若 renderAll 抛异常导致复位被跳过，
         * scheduled 会永久为 true，此后所有 scheduleRender 被拦截，
         * 渲染管线死锁 */
        scheduled = false;
      }
    });
  }

  /* ---- DOM 扫描：属性变化（虚拟列表复用节点改 data-turn-id /
   *      data-running）与子树增删统一走全量重渲（对话区节点量小，
   *      单次遍历开销可忽略，rAF 合并避免逐帧重复渲染） ---- */
  try {
    new MutationObserver(scheduleRender).observe(document.body, {
      childList: true,
      subtree: true,
      attributes: true,
      attributeFilter: [ATTR_TURN_ID, ATTR_RUNNING]
    });
  } catch (e) {
    /* 观察器不可用时仅失去动态刷新，静默 */
  }

  ensureStyle();
  /* V13：会话条 absolute 挂进输入区容器、随文档流自适应，无需 V12 的
   * window resize 重定位监听（已删除） */
  scheduleRender(); /* 立即渲染已挂载的轮次，不等首个数据周期 */
  pollLoop();
  /* 死锁/漏渲染兜底：低频定时器不经过 scheduled 检查直接调用一次
   * renderAll——即使未来出现新的意外状态（调度标志卡死、事件丢失等）
   * 也能在一个兜底周期内自愈，恢复统计条显示 */
  setInterval(function () {
    try {
      renderAll();
    } catch (e) {
      /* 兜底轮失败静默，下个周期再试 */
    }
  }, FALLBACK_RENDER_MS);
})();
"#;

// ============================================================
// URL 工具
// ============================================================

/// 文件路径 → file:// URL 的 percent-encoding。
/// 保留 RFC3986 unreserved（A-Za-z0-9-._~）与路径分隔符 `/`，
/// 其余字节（空格、中文等）按 %XX 编码，避免 Electron 加载外链失败。
pub fn percent_encode_path(path: &str) -> String {
    let mut out = String::with_capacity(path.len());
    for b in path.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' => {
                out.push(*b as char);
            }
            _ => {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

/// 剥离 Windows `Path::canonicalize` 返回的 verbatim 前缀：
/// `\\?\C:\dir\…` → `C:\dir\…`，`\\?\UNC\server\share\…` → `\\server\share\…`；
/// 非 Windows 或无前缀时原样返回。canonicalize 的结果统一过一遍，
/// 避免 verbatim 路径流入 params.json 与 file:// URL（verbatim 斜杠化后
/// 形如 `//?/C:/…`，Chromium 无法加载）。
pub(crate) fn strip_verbatim_prefix(path: PathBuf) -> PathBuf {
    #[cfg(windows)]
    {
        let text = path.as_os_str().to_string_lossy();
        if let Some(rest) = text.strip_prefix(r"\\?\UNC\") {
            return PathBuf::from(format!(r"\\{rest}"));
        }
        if let Some(rest) = text.strip_prefix(r"\\?\") {
            return PathBuf::from(rest.to_string());
        }
        path
    }
    #[cfg(not(windows))]
    {
        path
    }
}

/// 绝对路径 → file:// URL（Unix 形如 file:///Users/…，Windows 形如 file:///C:/…）。
/// Windows 分支拼接前剥离可能残留的 `\\?\` verbatim 前缀，兜住历史脏数据；
/// POSIX 绝对路径（"/" 开头，如从 mac 迁移的 params.json）自身已含根斜杠，
/// 固定前缀降为 file:// 避免拼出 file://// 四斜杠畸形 URL。
pub fn file_url(path: &Path) -> String {
    #[cfg(windows)]
    {
        let unified = strip_verbatim_prefix(path.to_path_buf())
            .to_string_lossy()
            .replace('\\', "/");
        let prefix = if unified.starts_with('/') {
            "file://"
        } else {
            "file:///"
        };
        format!("{}{}", prefix, percent_encode_path(&unified))
    }
    #[cfg(not(windows))]
    {
        format!("file://{}", percent_encode_path(&path.to_string_lossy()))
    }
}

// ============================================================
// variables.css 生成
// ============================================================

/// 由主题参数渲染 variables.css 内容。
/// 壁纸 URL 为 file:// 绝对地址（已 percent-encoding）。
/// --zbar-base-alpha 为全局氛围底透明度（V10 起由用户参数 base_alpha
/// 渲染真值，默认 0.25）：theme.css V5 起全部全局底色 token 由它驱动，
/// 与滑块解绑。
/// --zbar-text-shadow 为文字描边强度（V10 新增，由用户参数
/// text_shadow 渲染真值，0=关闭）：theme.css V10 起三区域主容器的
/// 前景文字描边由它驱动。两者均随 variables.css 每秒热重载即时生效。
/// --zbar-usage-font-size / --zbar-usage-opacity 为对话内用量统计条的
/// 字号与文字不透明度（V4 新增，由用户参数 usage_font_size /
/// usage_opacity 渲染真值）：usage.js V4 起统计条样式消费这两个变量
/// （模板内仅保留原写死值兜底），同样随每秒热重载即时生效。
/// --zbar-usage-session-bar 为会话级实时统计条的开关（V5 新增，由用户
/// 参数 usage_session_bar 渲染 1/0）：usage.js V5 起读它决定是否渲染
/// 会话条（变量缺失视为开启，与默认值 true 一致），改开关约 1 秒随
/// 热重载生效。
/// --zbar-usage-turn-bar 为每轮末尾统计条的开关（V19 新增，由用户参数
/// usage_turn_bar 渲染 1/0）：usage.js V19 起 renderAll 第二遍渲染前读它
/// 决定是否渲染每轮条（变量缺失视为开启，与默认值 true 一致），同样随
/// 每秒热重载即时生效。
pub fn render_variables_css(params: &ThemeParams, wallpaper_url: &str) -> String {
    format!(
        "/* ZBar 自动生成的主题变量，请勿手工编辑 */\n\
         /* 修改参数请在 ZBar 面板的动态壁纸设置中进行 */\n\
         :root {{\n\
         \x20 --zbar-wallpaper-url: url(\"{url}\");\n\
         \x20 --zbar-wp-brightness: {brightness};\n\
         \x20 --zbar-wp-saturate: {saturate};\n\
         \x20 --zbar-wp-blur: {blur}px;\n\
         \x20 --zbar-mask-strength: {mask};\n\
         \x20 --zbar-panel-opacity: {panel};\n\
         \x20 --zbar-sidebar-opacity: {sidebar};\n\
         \x20 --zbar-sidebar-right-opacity: {sidebar_right};\n\
         \x20 --zbar-base-alpha: {base_alpha};\n\
         \x20 --zbar-text-shadow: {text_shadow};\n\
         \x20 --zbar-usage-font-size: {usage_font_size}px;\n\
         \x20 --zbar-usage-opacity: {usage_opacity};\n\
         \x20 --zbar-usage-session-bar: {usage_session_bar};\n\
         \x20 --zbar-usage-turn-bar: {usage_turn_bar};\n\
         \x20 --zbar-playback-rate: {rate};\n\
         }}\n",
        url = wallpaper_url,
        brightness = params.wp_brightness,
        saturate = params.wp_saturate,
        blur = params.wp_blur,
        mask = params.mask_strength,
        panel = params.panel_opacity,
        sidebar = params.sidebar_opacity,
        sidebar_right = params.sidebar_right_opacity,
        base_alpha = params.base_alpha,
        text_shadow = params.text_shadow,
        usage_font_size = params.usage_font_size,
        usage_opacity = params.usage_opacity,
        usage_session_bar = if params.usage_session_bar { 1 } else { 0 },
        usage_turn_bar = if params.usage_turn_bar { 1 } else { 0 },
        rate = params.playback_rate,
    )
}

// ============================================================
// 注入块操作
// ============================================================

/// html 中是否已含注入标记（用于安装后抽检与状态检测）
pub fn has_inject(html: &str) -> bool {
    html.contains(INJECT_BEGIN)
}

/// 剥离所有旧的注入标记块（BEGIN…END 之间的全部内容连同标记，
/// 及块尾紧随的换行），保证重复安装幂等且还原后无残留空行；
/// 孤立的 BEGIN 标记（无 END 闭合）也一并清除。
pub fn strip_inject_blocks(html: &str) -> String {
    let mut out = html.to_string();
    loop {
        let Some(b) = out.find(INJECT_BEGIN) else {
            break;
        };
        match out[b..].find(INJECT_END) {
            Some(rel) => {
                let mut e = b + rel + INJECT_END.len();
                // 块尾紧随的换行一并移除，避免剥离后残留空行
                if out[e..].starts_with('\n') {
                    e += 1;
                }
                out.replace_range(b..e, "");
            }
            None => {
                // 不完整的残留块：仅移除标记本身
                out.replace_range(b..b + INJECT_BEGIN.len(), "");
            }
        }
    }
    out
}

/// 大小写不敏感查找（仅用于 ASCII 标签 </head> / </body>；
/// to_ascii_lowercase 字节一一对应，索引可直接用于原串）
fn find_ignore_case(haystack: &str, needle: &str) -> Option<usize> {
    haystack
        .to_ascii_lowercase()
        .find(&needle.to_ascii_lowercase())
}

/// 在 tag 前插入 block
fn insert_before(html: &str, tag: &str, block: &str, tag_desc: &str) -> Result<String, String> {
    match find_ignore_case(html, tag) {
        Some(i) => Ok(format!("{}{}{}", &html[..i], block, &html[i..])),
        None => Err(format!("index.html 结构异常：未找到 {tag_desc}（{tag}），已中止注入")),
    }
}

/// 向解包后的 index.html 注入主题外链引用。
/// 幂等：先剥离旧注入块，再插入新块：
///   - variables.css + theme.css（带 data-zbar-variables / data-zbar-theme
///     标记，供 effects.js 定位热重载目标）于 `</head>` 前
///   - effects.js（defer，带 data-zbar-effects 标记）+
///     usage.js（defer，带 data-zbar-usage 标记，对话页用量统计条）于
///     `</body>` 前
/// 返回写回后的完整 html。
pub fn apply_inject(staging_index_html: &Path, theme_dir: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(staging_index_html)
        .map_err(|e| format!("读取 index.html 失败: {e}"))?;
    let cleaned = strip_inject_blocks(&raw);

    let vars_url = file_url(&theme_dir.join(crate::agent_theme::store::VARIABLES_CSS));
    let theme_url = file_url(&theme_dir.join(crate::agent_theme::store::THEME_CSS));
    let effects_url = file_url(&theme_dir.join(crate::agent_theme::store::EFFECTS_JS));
    let usage_url = file_url(&theme_dir.join(crate::agent_theme::store::USAGE_JS));

    let head_block = format!(
        "{INJECT_BEGIN}\n<link rel=\"stylesheet\" href=\"{vars_url}\" data-zbar-variables=\"\">\n<link rel=\"stylesheet\" href=\"{theme_url}\" data-zbar-theme=\"\">\n{INJECT_END}\n"
    );
    let body_block = format!(
        "{INJECT_BEGIN}\n<script defer src=\"{effects_url}\" data-zbar-effects=\"\"></script>\n<script defer src=\"{usage_url}\" data-zbar-usage=\"\"></script>\n{INJECT_END}\n"
    );

    let mut html = insert_before(&cleaned, "</head>", &head_block, "文档头结束标签")?;
    html = insert_before(&html, "</body>", &body_block, "文档体结束标签")?;

    fs::write(staging_index_html, &html).map_err(|e| format!("写回 index.html 失败: {e}"))?;
    Ok(html)
}

// ============================================================
// 单元测试
// ============================================================

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn test_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!(
            "zbar-agent-theme-inject-test-{}-{name}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&d);
        fs::create_dir_all(&d).unwrap();
        d
    }

    const SAMPLE_HTML: &str = "<!DOCTYPE html>\n<html>\n<head>\n<meta charset=\"utf-8\">\n<title>ZCode</title>\n</head>\n<body>\n<div id=\"root\"></div>\n</body>\n</html>\n";

    #[test]
    fn 注入幂等_二次注入不多插() {
        let dir = test_dir("idempotent");
        let index = dir.join("index.html");
        fs::write(&index, SAMPLE_HTML).unwrap();

        // 第一次注入：head + body 各一个标记块
        let html1 = apply_inject(&index, &dir).unwrap();
        assert!(has_inject(&html1));
        assert_eq!(html1.matches(INJECT_BEGIN).count(), 2);
        assert!(html1.contains("</head>"));
        assert!(html1.contains("</body>"));

        // 第二次注入（对同一文件重复执行）：仍是 2 个标记块，不多插
        let html2 = apply_inject(&index, &dir).unwrap();
        assert_eq!(html2.matches(INJECT_BEGIN).count(), 2, "二次注入不应产生多余标记块");
        assert_eq!(html2.matches(INJECT_END).count(), 2);
        assert_eq!(html2.matches("variables.css").count(), 1);
        assert_eq!(html2.matches("effects.js").count(), 1);
        assert_eq!(html2.matches("usage.js").count(), 1, "二次注入不应重复 usage.js");

        // 引用位置：样式在 </head> 之前、脚本在 </body> 之前
        let head_pos = html2.to_ascii_lowercase().find("</head>").unwrap();
        let vars_pos = html2.find("variables.css").unwrap();
        assert!(vars_pos < head_pos, "样式链接应位于 </head> 之前");
        let body_pos = html2.to_ascii_lowercase().find("</body>").unwrap();
        let js_pos = html2.find("effects.js").unwrap();
        assert!(js_pos < body_pos, "脚本应位于 </body> 之前（defer）");
        let usage_pos = html2.find("usage.js").unwrap();
        assert!(usage_pos < body_pos, "usage.js 应位于 </body> 之前（defer）");
        assert!(html2.contains("<script defer src="));

        // 剥离后应还原为无标记的原貌
        let stripped = strip_inject_blocks(&html2);
        assert!(!has_inject(&stripped));
        assert_eq!(stripped, SAMPLE_HTML);

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 注入幂等_历史残留块被清理() {
        // 预置含旧注入块的 html（模拟升级重装场景）
        let dirty = format!(
            "<html><head><title>t</title>{BEGIN}<link rel=\"stylesheet\" href=\"file:///old.css\">{END}</head><body>{BEGIN}<script src=\"file:///old.js\"></script>{END}</body></html>",
            BEGIN = INJECT_BEGIN,
            END = INJECT_END
        );
        let dir = test_dir("legacy");
        let index = dir.join("index.html");
        fs::write(&index, dirty).unwrap();

        let html = apply_inject(&index, &dir).unwrap();
        assert_eq!(html.matches(INJECT_BEGIN).count(), 2);
        assert!(!html.contains("old.css"));
        assert!(!html.contains("old.js"));
        assert!(html.contains("variables.css"));

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn 注入_缺少闭合标签时报中文错误() {
        let dir = test_dir("malformed");
        let index = dir.join("index.html");
        fs::write(&index, "<html><head></head><body>no tail</body>").unwrap();
        // 该 html 缺 </body>？——上面其实含 </body>，去掉它构造异常结构
        fs::write(&index, "<html><head></head><body>broken").unwrap();
        let err = apply_inject(&index, &dir).unwrap_err();
        assert!(err.contains("index.html 结构异常"), "错误信息应说明结构异常：{err}");

        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn percent_编码_空格与中文() {
        assert_eq!(
            percent_encode_path("/Users/a b/中文.mp4"),
            "/Users/a%20b/%E4%B8%AD%E6%96%87.mp4"
        );
        // unreserved 与路径分隔符保持原样
        assert_eq!(percent_encode_path("/opt/homebrew/bin/npx-v1.2_3~"), "/opt/homebrew/bin/npx-v1.2_3~");
        // file_url 形态
        let url = file_url(Path::new("/Users/z/我的壁纸.mp4"));
        assert!(url.starts_with("file:///Users/z/"));
        assert!(url.contains("%E6%88%91%E7%9A%84%E5%A3%81%E7%BA%B8.mp4"));
    }

    #[cfg(windows)]
    #[test]
    fn verbatim前缀剥离_盘符UNC与无前缀形态() {
        // 盘符 verbatim：\\?\C:\… → C:\…
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\C:\Users\z\w.mp4")),
            PathBuf::from(r"C:\Users\z\w.mp4")
        );
        // UNC verbatim：\\?\UNC\srv\share → \\srv\share
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"\\?\UNC\srv\share\w.mp4")),
            PathBuf::from(r"\\srv\share\w.mp4")
        );
        // 无前缀原样返回（普通绝对路径与相对文件名都不受影响）
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from(r"C:\Users\z\w.mp4")),
            PathBuf::from(r"C:\Users\z\w.mp4")
        );
        assert_eq!(
            strip_verbatim_prefix(PathBuf::from("a.mp4")),
            PathBuf::from("a.mp4")
        );
    }

    #[cfg(windows)]
    #[test]
    fn file_url_verbatim路径兜底剥前缀() {
        // 历史 params.json 存过 \\?\C:\… 形态，重渲时必须自愈为合法 URL
        let url = file_url(Path::new(r"\\?\C:\Users\z\wp\my wall.mp4"));
        assert_eq!(url, "file:///C%3A/Users/z/wp/my%20wall.mp4");
    }

    #[test]
    fn variables_css_渲染含全部变量与壁纸地址() {
        let params = ThemeParams::default();
        let url = "file:///Users/z/.zbar/agent-themes/zcode/wallpapers/my%20wall.mp4";
        let css = render_variables_css(&params, url);
        for var in [
            "--zbar-wallpaper-url",
            "--zbar-wp-brightness",
            "--zbar-wp-saturate",
            "--zbar-wp-blur",
            "--zbar-mask-strength",
            "--zbar-panel-opacity",
            "--zbar-sidebar-opacity",
            "--zbar-sidebar-right-opacity",
            "--zbar-base-alpha",
            "--zbar-text-shadow",
            "--zbar-usage-font-size",
            "--zbar-usage-opacity",
            "--zbar-usage-session-bar",
            "--zbar-usage-turn-bar",
            "--zbar-playback-rate",
        ] {
            assert!(css.contains(var), "variables.css 缺少变量 {var}");
        }
        // 壁纸地址以 url("…") 形式写入
        assert!(css.contains(&format!("url(\"{url}\")")));
        // 默认参数值渲染（V6 默认：亮度/饱和度拉满、对话列/侧栏/右栏
        // 归零、氛围底默认 0.25、V10 描边默认关闭、速率 1；用量条字号
        // 10px / 不透明度 0.55 与 V3 usage.js 模板写死观感一致）
        assert!(css.contains("--zbar-wp-brightness: 1.1;"));
        assert!(css.contains("--zbar-wp-saturate: 1.4;"));
        assert!(css.contains("--zbar-wp-blur: 0px;"));
        assert!(css.contains("--zbar-mask-strength: 0;"));
        assert!(css.contains("--zbar-panel-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-right-opacity: 0;"));
        assert!(css.contains("--zbar-base-alpha: 0.25;"));
        assert!(css.contains("--zbar-text-shadow: 0;"));
        assert!(css.contains("--zbar-usage-font-size: 10px;"));
        assert!(css.contains("--zbar-usage-opacity: 0.55;"));
        assert!(css.contains("--zbar-usage-session-bar: 1;"), "{css}");
        assert!(css.contains("--zbar-usage-turn-bar: 1;"), "{css}");
        assert!(css.contains("--zbar-playback-rate: 1;"));
        // 非 ASCII 壁纸名在 url 里必须已编码（不出现裸中文）
        assert!(!css.contains("我的壁纸"));

        // V10：两个新参数按参数渲染真值（不再写死常量），改参后
        // variables.css 跟随变化（热重载即时生效的前提）
        let mut p = ThemeParams::default();
        p.base_alpha = 0.4;
        p.text_shadow = 0.8;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-base-alpha: 0.4;"), "{css}");
        assert!(css.contains("--zbar-text-shadow: 0.8;"), "{css}");

        // V4：用量统计条参数同样按参数渲染真值（皮肤页滑块改参后
        // 随 variables.css 热重载即时生效的前提）
        let mut p = ThemeParams::default();
        p.usage_font_size = 13.0;
        p.usage_opacity = 0.8;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-usage-font-size: 13px;"), "{css}");
        assert!(css.contains("--zbar-usage-opacity: 0.8;"), "{css}");

        // V5：会话累计条开关按参数渲染 1/0（关闭时 usage.js 读到 0
        // 即不渲染会话条，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_session_bar = false;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-usage-session-bar: 0;"), "{css}");
        p.usage_session_bar = true;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-usage-session-bar: 1;"), "{css}");

        // V19：每轮统计条开关同样按参数渲染 1/0（关闭时 renderAll 对
        // 全部轮节点 removeRow 并跳过 renderOne，随热重载约 1 秒生效）
        let mut p = ThemeParams::default();
        p.usage_turn_bar = false;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-usage-turn-bar: 0;"), "{css}");
        p.usage_turn_bar = true;
        let css = render_variables_css(&p, url);
        assert!(css.contains("--zbar-usage-turn-bar: 1;"), "{css}");
    }

    #[test]
    fn has_inject_判定() {
        assert!(!has_inject(SAMPLE_HTML));
        assert!(has_inject(&format!("x{INJECT_BEGIN}y{INJECT_END}z")));
    }

    #[test]
    fn 模板_版本头与token覆盖与图片支持() {
        // 头部版本标记（store::ensure_versioned_template 的升级判据）：
        // theme.css 升 V10（文字可读性增强 + 氛围底可调），
        // effects.js V5（撤销 theme.css 每秒热重载 + poll 快照空值防御）
        assert!(THEME_CSS.contains("ZBAR-THEME-V11"));
        assert!(!THEME_CSS.contains("ZBAR-THEME-V9"), "版本头应已升到 V10");
        assert!(EFFECTS_JS.contains("ZBAR-THEME-V5"));
        assert!(!EFFECTS_JS.contains("ZBAR-THEME-V4"), "版本头应已升到 V5");
        // usage.js V19（新增每轮统计条开关参数 usage_turn_bar（默认
        // 开启）：renderAll 第二遍渲染前统一读 --zbar-usage-turn-bar（变
        // 量缺失视为开启），关闭时对全部轮节点 removeRow 并跳过 renderOne，
        // syncDyn 与会话条不受影响）；V18 修复新建任务后（空会话）会话
        // 累计条停留在上一个
        // 会话数据：renderAll 无轮节点分支早退导致 renderSessionBar 永
        // 不执行，该分支补 removeBar；V17 修复新建任务后会话累计条不消
        // 失、数据不重置：currentSessionId 改为焦点优先+可见优先的容器
        // 遍历选择（旧
        // querySelector 首中在多会话保活时拿到旧会话），composer 挂载
        // 点经 pickComposerRegion 可见优先并跟随当前会话容器；V16 整体
        // 移除每轮统计行的悬浮 title 提示，titleOf/
        // liveTitleOf/LIVE_TITLE/LIVE_START_TITLE 一并删除；V15 会话条
        // Σ 段新增会话总 Token + 速度段去 ⋯ 前缀 +
        // 流式估算段改 ≈ 前缀；V14 修复 V13 遗漏 SEL_COMPOSER 定义 + 挂
        // 载异常一次性告警；V13 会话条挂载进输入区容器 + CSS 留白定位，
        // V12 动态测量废弃；V10 统计条显示稳定性：三态统一固定
        // 结构 + 数字等宽补位 + 会话条动态段固定 + 完成子代理面板残留
        // 占位枯萎清理；V9 子代理消耗实时化：document 级扫描 + 多容器
        // 活动轮 + 主轮 live 行 sub 合计 + 会话条 Σ 跳过 m 行；V8 启动
        // 窗口实时渲染；V7 请求图标 ⟳ → ×；V6 生成过程实时跳动）
        assert!(USAGE_JS.contains("ZBAR-THEME-V19"));
        assert!(!USAGE_JS.contains("ZBAR-THEME-V18"), "版本头应已升到 V19");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V17"), "版本头不应回退");
        assert!(!USAGE_JS.contains("ZBAR-THEME-V10"), "版本头不应回退");
        // V19 每轮统计条开关特征：开关变量 + 镜像读取函数 + renderAll
        // 第二遍渲染循环的关闭分支（对全部轮节点 removeRow 并跳过
        // renderOne）
        assert!(
            USAGE_JS.contains("VAR_TURN_BAR = \"--zbar-usage-turn-bar\""),
            "每轮统计条开关应读 --zbar-usage-turn-bar 变量（variables.css 渲染 1/0）"
        );
        assert!(
            USAGE_JS.contains("function turnBarEnabled"),
            "每轮统计条开关应有独立读取函数（镜像 sessionBarEnabled）"
        );
        assert!(
            USAGE_JS.contains("removeRow(nodes[i], id);"),
            "renderAll 关闭分支应对全部轮节点 removeRow 并跳过 renderOne"
        );
        // V18 空会话修复特征：renderAll 无轮节点早退分支必须调用
        // removeBar，否则新建任务/清空对话后会话条停留在上一个会话的
        // 累计值永不消失。切片范围：renderAll 函数体内空轮分支起点至
        // 该分支的 return 为止
        let ra_lo = USAGE_JS
            .find("function renderAll")
            .expect("renderAll 应存在");
        let ra_body = &USAGE_JS[ra_lo..];
        let br_off = ra_body
            .find("if (!nodes.length)")
            .expect("renderAll 空轮分支应存在");
        let br_len = ra_body[br_off..]
            .find("return;")
            .expect("空轮分支应包含 return");
        let empty_branch = &ra_body[br_off..br_off + br_len];
        assert!(
            empty_branch.contains("stopDyn()"),
            "空轮分支应保留 stopDyn（会话清空停估算目标）"
        );
        assert!(
            empty_branch.contains("removeBar();"),
            "V18 空轮分支应调用 removeBar（空会话时移除会话条，防残留上一会话累计）: {empty_branch}"
        );
        // V17 会话选择修复特征：可见性 helper（保活面板隐藏时跳过）、
        // composer 挂载点选择函数；旧 querySelector 首中写法应已删除
        assert!(
            USAGE_JS.contains("function visibleEl"),
            "V17 应新增可见性判断 helper（保活面板隐藏时 rect 归零或 display:none）"
        );
        assert!(
            USAGE_JS.contains("function pickComposerRegion"),
            "composer 挂载点应经 pickComposerRegion 可见优先并跟随当前会话容器"
        );
        assert!(
            !USAGE_JS
                .contains("(anchor || document).querySelector(SEL_SESSION_ID)"),
            "currentSessionId 旧 querySelector 首中写法应已删除（多会话保活时拿到旧会话）"
        );
        // V4 样式参数化特征（保留）：两个变量消费 + V3 写死值兜底 + 等待态乘系数
        assert!(
            USAGE_JS.contains("var(--zbar-usage-font-size,10px)"),
            "字号应消费 --zbar-usage-font-size 并以 V3 写死值兜底"
        );
        assert!(
            USAGE_JS.contains("opacity:var(--zbar-usage-opacity,.55)"),
            "正常态不透明度应消费 --zbar-usage-opacity 并以 V3 写死值兜底"
        );
        assert!(
            USAGE_JS.contains(
                "opacity:calc(var(--zbar-usage-opacity,.55)*.636)"
            ),
            "等待态低透明应由正常态乘 0.636 系数得出（默认 ≈0.35 同 V3）"
        );
        assert!(
            !USAGE_JS.contains("font:400 10px/1.5"),
            "font 简写应已拆分独立声明以参数化字号"
        );
        // V10 统一固定结构（沿袭 V5 图标：× 请求数 / t/s / TTFT）：
        // barLineOf 三态共用，全部字段位恒定渲染、等宽补位
        let lo = USAGE_JS.find("function barLineOf").expect("barLineOf 应存在");
        let hi = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let bar_body = &USAGE_JS[lo..hi];
        assert!(
            bar_body.contains("String(v.req).padStart(3, \" \")"),
            "req 应 padStart(3) 等宽补位：{bar_body}"
        );
        assert!(
            bar_body.contains("padSpeed(v.speed) + \" t/s\""),
            "输出速度应为 t/s 且经 padSpeed 补位：{bar_body}"
        );
        assert!(
            bar_body
                .contains("\" · TTFT \" + (v.ttft == null ? \"–\" : v.ttft).padStart(4, \" \")"),
            "首字延迟应显示为 TTFT 且 padStart(4)，进行中/缺失显示 – 占位：{bar_body}"
        );
        assert!(
            bar_body.contains("(v.est ? \"~\" : \" \")"),
            "↓ 估算前缀应为固定 1 字符占位位（进行中 ~ / 完成空格）：{bar_body}"
        );
        assert!(
            !bar_body.contains("+ \" req\"")
                && !bar_body.contains("\" tok/s\"")
                && !bar_body.contains("\"首字 \""),
            "旧格式渲染片段（N req / tok/s / 首字）不应残留：{bar_body}"
        );
        // V10 新特征：token 字段等宽补位、速度/TTFT 占位、极简启动行与
        // 行尾 … 标记删除
        assert!(
            USAGE_JS.contains("padStart(5, \" \")"),
            "token 字段应经 fmtTokens padStart(5) 等宽补位"
        );
        assert!(
            USAGE_JS.contains("function padSpeed") && USAGE_JS.contains("s == null ? \"  - \""),
            "速度应 padStart(4)，dur 缺失（老库）显示 4 字符占位"
        );
        assert!(
            USAGE_JS.contains("\"–\""),
            "TTFT 进行中/缺失应显示 – 占位"
        );
        assert!(
            !USAGE_JS.contains("function liveStartLineOf"),
            "启动窗口极简行函数应已删除（合并进统一格式函数）"
        );
        assert!(
            !USAGE_JS.contains("WAIT_TEXT"),
            "行尾 … 进行中标记应已删除（TTFT 位 – 已表达进行中）"
        );
        // 完成态：经统一格式函数渲染（est 前缀位为空格），不再逐段拼接；
        //（限定 lineOf 函数体检查——头部变更注释会合法提及旧格式）
        let lo = USAGE_JS.find("function lineOf").expect("lineOf 应存在");
        let hi = USAGE_JS
            .find("function liveLineOf")
            .expect("liveLineOf 应存在（lineOf 后下一个函数，V16 起 titleOf 已删）");
        let line_body = &USAGE_JS[lo..hi];
        assert!(
            line_body.contains("barLineOf({") && line_body.contains("est: false"),
            "完成态应经统一格式函数渲染（est 前缀位为空格）：{line_body}"
        );
        assert!(
            !line_body.contains("parts.push")
                && !line_body.contains("+ \" req\"")
                && !line_body.contains("\" tok/s\"")
                && !line_body.contains("\"首字 \""),
            "完成态逐段拼接与旧格式渲染片段不应残留：{line_body}"
        );
        // V5 会话级实时统计条特征：独立渲染函数 + 实机调参常量 + 防重复
        // 标记 + 会话锚点 + tabular-nums + pointer-events
        assert!(USAGE_JS.contains("renderSessionBar"), "会话条渲染函数应存在");
        assert!(
            USAGE_JS.contains("SESSION_BAR_BOTTOM_PX"),
            "兜底 bottom 实机调参常量应存在"
        );
        // V13 会话条挂载进输入区容器 + CSS 留白定位特征：region relative
        // + padding-top 留白规则、条 absolute 居中、幂等迁移挂载、兜底
        // fixed 属性切换；V11 输入区上移规则应已删除（输入框还原原位）
        assert!(
            USAGE_JS.contains(
                ".chat-composer-region{position:relative !important;padding-top:26px !important;}"
            ),
            "输入区容器应有 relative + 26px 顶部留白规则（与 COMPOSER_PAD_TOP_PX 同步）"
        );
        assert!(
            USAGE_JS.contains("COMPOSER_PAD_TOP_PX = 26"),
            "留白高度常量注释应存在（CSS 内写死 26px，两处同步）"
        );
        assert!(
            USAGE_JS.contains("position:absolute;top:4px;left:50%;transform:translateX(-50%);"),
            "会话条应 absolute 定位住进输入区容器顶部留白（不再 fixed/bottom）"
        );
        assert!(
            USAGE_JS.contains("region.appendChild(bar)")
                && USAGE_JS.contains("var SEL_COMPOSER = \".chat-composer-region\""),
            "会话条应幂等迁移挂载进输入区容器（SEL_COMPOSER 须有真实定义，注释提及不算）"
        );
        assert!(
            USAGE_JS.contains("ATTR_SESSION_BAR_FIXED"),
            "region 缺失兜底路径应有 fixed 切换标记属性"
        );
        assert!(
            USAGE_JS.contains("SESSION_BAR_BOTTOM_PX = 96"),
            "region 缺失退回 fixed 的兜底 bottom 应为 96"
        );
        // 会话条零测量定位：renderSessionBar 函数体内不得再有
        // getBoundingClientRect（V12 动态测量废弃；限定函数体范围断言
        // ——模板头 V12 历史注释合法提及该字样）
        let sb_lo = USAGE_JS
            .find("function renderSessionBar")
            .expect("renderSessionBar 应存在");
        let sb_hi = sb_lo
            + USAGE_JS[sb_lo..]
                .find("function renderAll")
                .expect("renderAll 应存在");
        let session_body = &USAGE_JS[sb_lo..sb_hi];
        assert!(
            !session_body.contains("getBoundingClientRect"),
            "会话条应零测量定位（V12 动态测量已废弃）：{session_body}"
        );
        assert!(
            !USAGE_JS.contains("addEventListener(\"resize\""),
            "absolute 随文档流自适应，V12 的 resize 监听应已删除"
        );
        assert!(
            !USAGE_JS.contains("var SESSION_BAR_ABOVE_PX"),
            "V12 测量间距常量定义应已删除"
        );
        assert!(
            !USAGE_JS.contains("var COMPOSER_GAP_PX")
                && !USAGE_JS.contains("chat-composer-region{padding-bottom"),
            "V11 输入区上移规则与 COMPOSER_GAP_PX 常量定义应已删除（输入框还原原位）"
        );
        assert!(USAGE_JS.contains("TOKEN_CHARS"), "token 估算系数常量应存在");
        assert!(USAGE_JS.contains("DYN_TICK_MS"), "动态段刷新周期常量应存在");
        assert!(USAGE_JS.contains("SPEED_WINDOW_MS"), "速度滑动窗口常量应存在");
        assert!(
            USAGE_JS.contains("data-zbar-usage-session"),
            "会话条防重复挂载标记应存在"
        );
        assert!(
            USAGE_JS.contains("[data-session-id]"),
            "会话 id 锚点选择器应存在"
        );
        assert!(
            USAGE_JS.contains("t.sess !== sessId"),
            "会话累计应按 sess 过滤 turns"
        );
        // V6 生成过程实时跳动特征：runIndex 数据索引、live 行渲染、
        // 数据驱动的估算目标检测、会话条 run 合计并入
        assert!(
            USAGE_JS.contains("runIndex[t.umid] = r") || USAGE_JS.contains("runIndex[r.umid] = r"),
            "应建立进行中轮 runIndex（umid → run 行）"
        );
        assert!(
            USAGE_JS.contains("function liveLineOf"),
            "进行中轮行渲染函数应存在"
        );
        assert!(
            !USAGE_JS.contains("LIVE_TITLE"),
            "V16 起进行中轮行不应再有悬浮 title 提示（已整体移除）"
        );
        assert!(
            USAGE_JS.contains("data-zbar-usage-row-state\", \"live\""),
            "进行中轮行应有独立 state 标记（与完成/等待态区分）"
        );
        assert!(
            USAGE_JS.contains("function findLiveNodes"),
            "活动轮判定/估算目标检测函数应存在（findLiveNodes，V9 多容器）"
        );
        assert!(
            USAGE_JS.contains("function sessionRunTotals"),
            "会话条应并入进行中 run 合计"
        );
        assert!(
            USAGE_JS.contains("r.psess !== sessId"),
            "子代理进行中轮应经 psess 并入父会话累计"
        );
        assert!(
            USAGE_JS.contains("(data && data.runs) || []"),
            "runs 缺失（旧导出文件）应兜底空数组"
        );
        assert!(
            USAGE_JS.contains("req: req,"),
            "进行中轮行应显示 run 真实请求数聚合（V9 起含并入的子代理 req，V10 经统一格式函数补位）"
        );
        // V8 启动窗口实时渲染特征（活动轮判定 DOM 驱动，修复发消息到
        // 首笔模型请求完成之间 live 条/估算目标/会话条全空白的启动窗口），
        // V9 多容器化：每容器取 DOM 顺序最后 umid 节点，不在 index 即该
        // 会话活动轮
        assert!(
            USAGE_JS.contains("if (!id || index[id]) continue"),
            "活动轮判定应为 DOM 驱动：会话 DOM 最后 umid 不在 index 即活动轮"
        );
        // 判定无 runs 依赖路径：findLiveNodes 函数体内不得引用 runIndex
        //（runIndex 是否命中只区分启动窗口/runs 阶段，不是判定前提），
        // 也不得依赖实机不可靠的 data-running
        let fl_lo = USAGE_JS
            .find("function findLiveNodes")
            .expect("findLiveNodes 应存在");
        let fl_hi = fl_lo
            + USAGE_JS[fl_lo..]
                .find("function ensureBar")
                .expect("ensureBar 应存在");
        let find_body = &USAGE_JS[fl_lo..fl_hi];
        assert!(
            !find_body.contains("runIndex["),
            "活动轮判定不得依赖 runs 数据（runIndex 命中不是前提）：{find_body}"
        );
        assert!(
            !find_body.contains("ATTR_RUNNING"),
            "活动轮判定不得依赖 data-running（实机取值存疑）：{find_body}"
        );
        // 启动窗口活动轮行：V10 起经统一格式函数渲染 0 / 估算值（极简
        // 行已删除）；renderOne 接收 renderAll 统一判定的活动轮 Map
        //（V9 多容器：会话 id → 活动轮节点）。V16 起无悬浮 title
        assert!(
            !USAGE_JS.contains("LIVE_START_TITLE"),
            "V16 起启动窗口活动轮行不应再有悬浮 title 提示（已整体移除）"
        );
        assert!(
            USAGE_JS.contains("function renderOne(node, turnId, isLast, activeMap)"),
            "renderOne 应接收统一判定的活动轮 Map（V9 多容器）"
        );
        // data-running 不再是任何渲染路径的必要条件：等待态判定与兜底
        // 选择器删除，仅保留 attributeFilter 刷新信号
        assert!(
            !USAGE_JS.contains("SEL_RUNNING_TURN"),
            "data-running 兜底选择器应已删除"
        );
        assert!(
            !USAGE_JS.contains("=== \"true\""),
            "不得再以 data-running 取值作为渲染必要条件"
        );
        assert!(
            USAGE_JS.contains("attributeFilter: [ATTR_TURN_ID, ATTR_RUNNING]"),
            "data-running 应仅保留为 MutationObserver 刷新信号"
        );
        // 会话条：放弃渲染条件追加"无活动轮"（启动窗口即时显示）；估算
        // 输出移入动态段 ≈est（V15 起前缀），不再叠加进 Σ ↓ 真实数字
        assert!(
            USAGE_JS.contains("!totals && !runTotals && !active"),
            "会话条放弃渲染条件应含无活动轮判定（启动窗口即时显示）"
        );
        assert!(
            USAGE_JS.contains("\"≈\" + fmtTokens(est.tok)"),
            "会话条动态段应含流式估算输出 ≈est（V15：↓ ~ 改 ≈ 前缀）"
        );
        // V10 动态段固定：两段永远显示（idle 无活动轮时速度 0.0、估算 0），
        // 不再按有无值省略，Σ 行整体宽度恒定；Σ 数字段同步补位。V15：
        // Σ 段为会话总 Token（tsum），速度段去 ⋯ 前缀、估算段 ≈ 前缀
        assert!(
            USAGE_JS.contains("var tsum = tin + tout + tcr;")
                && USAGE_JS.contains("parts.push(padSpeed(est.speed || 0) + \" t/s\");")
                && USAGE_JS.contains("parts.push(\"≈\" + fmtTokens(est.tok));"),
            "会话条动态段两段应固定显示（idle 时 0.0 / 0，不再按有无值省略）"
        );
        assert!(
            USAGE_JS.contains("String(treq).padStart(3, \" \")"),
            "会话条 Σ req 应 padStart(3) 等宽补位"
        );
        assert!(
            !USAGE_JS.contains("(runTotals ? runTotals.tout : 0) + est.tok"),
            "会话条 Σ ↓ 真实数字不得再叠加估算（估算已移入动态段）"
        );
        assert!(
            USAGE_JS.contains("VAR_SESSION_BAR = \"--zbar-usage-session-bar\""),
            "会话条开关应读 --zbar-usage-session-bar 变量（variables.css 渲染 1/0）"
        );
        assert!(
            USAGE_JS.contains("position:fixed"),
            "会话条兜底路径应保留 fixed 定位（region 缺失时挂 body 退回旧悬浮方式，不占消息布局）"
        );
        assert!(
            USAGE_JS.contains("pointer-events:none"),
            "会话条应 pointer-events:none 防挡输入区交互"
        );
        assert!(
            USAGE_JS.contains("font-variant-numeric:tabular-nums"),
            "会话条应 tabular-nums 防数字跳动宽度抖动"
        );
        // V6 估算口径特征：dynEstimate 返回 {tok,speed} 供消费端叠加；
        // 旧 "↓ ~Y" 独立估算段已移除（V5 历史变更注释保留提及，全文
        // 检索无意义，改查消费端拼串特征）
        assert!(
            USAGE_JS.contains("function dynEstimate"),
            "流式估算应收敛为 dynEstimate（tok/speed 叠加口径）"
        );
        assert!(
            USAGE_JS.contains("EST_ZERO"),
            "非估算节点应有空估算常量兜底"
        );
        assert!(
            !USAGE_JS.contains("function dynSegmentText"),
            "V5 的 dynSegmentText 拼串应已被 dynEstimate 取代"
        );
        assert!(
            USAGE_JS.contains("clearInterval"),
            "动态定时器结束时必须清除（防泄漏）"
        );
        // V9 子代理消耗实时化特征：document 级扫描（子代理详情面板与主
        // 对话同 document，面板在 workspace-main 锚点之外）、多容器活动
        // 轮判定、估算器多目标统一采样、主轮 live 行 sub 合计、会话条
        // Σ 跳过 m:1 子代理行并计入主行 sub
        assert!(
            USAGE_JS.contains("var nodes = document.querySelectorAll(SEL_TURN)"),
            "renderAll 应 document 级扫描每轮节点（覆盖锚点外的子代理面板）"
        );
        assert!(
            USAGE_JS.contains("containers[i].querySelectorAll(SEL_TURN)")
                && USAGE_JS.contains("new Map()"),
            "活动轮判定应遍历所有会话容器并以 Map 返回（sess → 活动轮节点）"
        );
        assert!(
            USAGE_JS.contains("dyn.targets") && USAGE_JS.contains("function syncDyn"),
            "估算器应支持多目标（sess → 采样状态）并统一管理"
        );
        assert!(
            USAGE_JS.contains("function estFor") && USAGE_JS.contains("function sessOf"),
            "估算应按节点所属会话取目标（estFor/sessOf）"
        );
        assert!(
            USAGE_JS.contains("var s = r.sub;")
                && USAGE_JS.contains("req: req,")
                && USAGE_JS.contains("est: true"),
            "主轮 live 行数字段应合计并入的子代理聚合 sub（V10 经统一格式函数渲染）"
        );
        assert!(
            !USAGE_JS.contains("liveTitleOf")
                && !USAGE_JS.contains("\"含子代理 \" + r.sub.n + \" 轮：\""),
            "V16 起每轮行不应再有悬浮 title（liveTitleOf 及子代理分解明细应已删除）"
        );
        assert!(
            USAGE_JS.contains("if (!r || r.m) continue;"),
            "会话条 runs 合计应跳过 m:1 子代理行（其值已并入主轮行 sub 防双计）"
        );
        assert!(
            USAGE_JS.contains("if (r.sub) {"),
            "会话条应把主轮行并入的子代理 sub 计入 Σ"
        );
        assert!(
            USAGE_JS.contains("var active = activeMap.has(sessId);"),
            "会话条活动轮判定应按本会话 id 从活动轮 Map 取（V9 多容器）"
        );
        assert!(
            !USAGE_JS.contains("dyn.node")
                && !USAGE_JS.contains("function startDyn(")
                && !USAGE_JS.contains("function findLiveNode("),
            "V8 单目标估算/单节点活动轮判定的旧接口应已移除"
        );
        // V10 枯萎判定：修复已完成子代理面板的残留占位——活动轮目标文
        // 本连续 STALE_MS 无增长且该 umid 始终不在 index/runIndex（值已
        // 并入主轮 sub，turns/runs 永无该行）→ removeRow 并从活动轮目
        // 标中移除；文本再变化时消费端重新评估
        assert!(
            USAGE_JS.contains("STALE_MS = 90000"),
            "枯萎判定阈值常量应存在（模板头部常量区）"
        );
        assert!(
            USAGE_JS.contains("function staleHolds")
                && USAGE_JS.contains("!index[tid] && !runIndex[tid]"),
            "枯萎判定应为文本连续无增长且 umid 始终不在 index/runIndex"
        );
        // 用量条核心特征：同目录数据文件推导 + 统计口径与等待态
        assert!(USAGE_JS.contains("usage-data.js"));
        assert!(USAGE_JS.contains("genMs"));
        assert!(USAGE_JS.contains("ttft * 10 >= d * 9"), "整块下发口径应与 db.rs 一致");
        assert!(USAGE_JS.contains("data-zbar-usage-row"), "统计条防重复标记应存在");
        // V2 实机修复特征：数据版本校验、umid 匹配键、DOM 顺序最后节点
        // 渲染、ts 未变跳过重建与重渲染
        assert!(USAGE_JS.contains("data.v !== 2"), "数据版本校验应存在");
        assert!(USAGE_JS.contains("index[t.umid] = t"), "索引应以 umid 为匹配键");
        assert!(
            USAGE_JS.contains("lastOf[id] === nodes[i]"),
            "同轮多节点应只在 DOM 顺序最后一个节点渲染"
        );
        assert!(USAGE_JS.contains("data.ts === lastTs"), "ts 未变应跳过重渲染");
        // V3 实机修复特征：scheduled 复位移入 finally（renderAll 抛异常
        // 不再死锁渲染管线）+ 低频兜底渲染（不经过 scheduled 检查直接重渲）
        assert!(
            USAGE_JS.contains("} finally {"),
            "scheduleRender 复位应在 finally 中，保证异常不死锁"
        );
        assert!(
            USAGE_JS.contains("FALLBACK_RENDER_MS"),
            "低频兜底渲染定时器应存在"
        );

        // token 半透明化：浅色 :root 与深色 .dark 两套均定义
        // background / background-alt / panel 三个 token
        assert!(THEME_CSS.contains(".dark"));
        for token in [
            "--color-background:",
            "--color-background-alt:",
            "--color-panel:",
        ] {
            assert_eq!(
                THEME_CSS.matches(token).count(),
                2,
                "{token} 应在 :root 与 .dark 各定义一次"
            );
        }
        // 引用 neutral 原色不硬编码
        assert!(THEME_CSS.contains("var(--color-neutral-50)"));
        assert!(THEME_CSS.contains("var(--color-neutral-100)"));
        assert!(THEME_CSS.contains("var(--color-neutral-800)"));
        assert!(THEME_CSS.contains("var(--color-neutral-900)"));

        // V5 全局 token 分层：8 条全局底色 token（:root 与 .dark 各 4 条）
        // 全部改由固定氛围变量 --zbar-base-alpha（缺省兜底 0.25）驱动，
        // 与对话区/侧栏两个滑块彻底解绑，拖任一滑块不再牵连顶栏、
        // 右侧面板、卡片等全局底消费方
        assert_eq!(
            THEME_CSS.matches("var(--zbar-base-alpha, 0.25)").count(),
            8,
            "全局底色 token 应在 :root 与 .dark 各 4 条消费 --zbar-base-alpha"
        );
        assert!(
            !THEME_CSS.contains("var(--zbar-panel-opacity, 0.85)")
                && !THEME_CSS.contains("var(--zbar-sidebar-opacity, 0.85)"),
            "全局 token 不得再由滑块变量驱动"
        );
        // 每条全局 token 声明的取值只消费 --zbar-base-alpha，
        // 不得混入任何滑块变量（全局底与滑块彻底解绑）
        for token in [
            "--color-background:",
            "--color-background-alt:",
            "--color-panel:",
            "--color-sidebar:",
        ] {
            let mut rest = THEME_CSS;
            for _ in 0..2 {
                let start = rest.find(token).unwrap();
                let tail = &rest[start..];
                let value = &tail[..tail.find(';').unwrap()];
                assert!(
                    value.contains("var(--zbar-base-alpha, 0.25)"),
                    "{token} 应由固定氛围变量驱动：{value}"
                );
                assert!(
                    !value.contains("--zbar-panel-opacity")
                        && !value.contains("--zbar-sidebar-opacity"),
                    "{token} 不得消费滑块变量：{value}"
                );
                rest = &tail[value.len()..];
            }
        }

        // V6 三滑块唯一作用区：侧栏滑块变量只由侧栏容器元素规则在浅色、
        // 深色各消费一次；对话区滑块变量只由对话列元素规则各消费一次；
        // 右栏滑块变量只由右栏其余面板元素规则各消费一次
        // （--color-sidebar 已改由氛围变量驱动，不再消费滑块变量）
        assert_eq!(
            THEME_CSS.matches("--color-sidebar:").count(),
            2,
            "--color-sidebar 应在 :root 与 .dark 各定义一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-sidebar-opacity, 0)").count(),
            2,
            "侧栏滑块变量应只由侧栏容器元素规则在浅色、深色各消费一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-panel-opacity, 0)").count(),
            2,
            "对话区滑块变量应只由对话列元素规则在浅色、深色各消费一次"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-sidebar-right-opacity, 0)").count(),
            2,
            "右栏滑块变量应只由右栏其余面板元素规则在浅色、深色各消费一次"
        );
        assert!(THEME_CSS.contains("var(--color-neutral-950)"));

        // V9 主区域元素级规则 + V10 描边规则（选择器同批复用）：
        // 侧栏容器浅色、深色各 2 条（背景 1 + 描边 1）；对话区四面板
        // 组与右栏组同理
        assert_eq!(
            THEME_CSS.matches("#sidebar").count(),
            4,
            "侧栏容器应在浅深各 2 条规则（V9 背景 + V10 描边）"
        );
        // V6 修正：#content 实为"对话列 + 右侧面板"整体容器，其元素规则
        // 已删除，对话区滑块不再作用其上
        assert_eq!(
            THEME_CSS.matches("#content").count(),
            0,
            "#content 元素规则应已移除（否则会牵连右侧面板）"
        );
        // V9 对话区四面板组：四个面板值各出现 8 次——V9 对话区规则
        // 浅深各 1 次 + V9 右栏 :not 链浅深各 1 次 + V10 描边规则
        // （对话区段与 :not 链同批复用）浅深各 2 次（缺一即归属串味）
        for pane in [
            "workspace-main",
            "conversation-column",
            "conversation",
            "terminal",
        ] {
            let needle = format!("{pane}\"]");
            assert_eq!(
                THEME_CSS.matches(&needle).count(),
                8,
                "面板值 {pane} 应在 V9 背景规则、右栏 :not 链与 V10 描边规则中浅深共 8 次"
            );
        }
        // 对话区规则与右栏规则以各自选择器末段形态区分：后跟 " {" 的
        // terminal 选择器是对话区组末段（V10 描边组中 terminal 后随
        // 逗号、不新增该形态），右栏组末段是空态选择面板容器类
        assert_eq!(
            THEME_CSS.matches("terminal\"] {").count(),
            2,
            "对话区四面板组元素规则应在浅色与深色各一条"
        );
        assert_eq!(
            THEME_CSS.matches(".side-pane-open-tab-shell {").count(),
            4,
            "右栏元素规则应在浅深各 2 条（V9 背景 + V10 描边，:not 链 + 空态选择面板容器类）"
        );
        // V7 修正：选择器属性名为 data-pane-id（按带括号的选择器形态
        // 计数，注释中的裸属性名不影响结果），旧写法须在选择器中归零
        assert_eq!(
            THEME_CSS.matches("[data-panel-id").count(),
            0,
            "旧属性名 data-panel-id 不应再出现在任何选择器中"
        );
        assert_eq!(
            THEME_CSS.matches("[data-pane-id").count(),
            36,
            "V9 对话区 2 条规则各 4 次 + 右栏 2 条规则各 5 次 = 18；V10 描边浅深两组结构相同再 +18"
        );
        assert!(THEME_CSS.contains("html.dark #sidebar"));
        assert!(THEME_CSS.contains("html.dark [data-pane-id=\"workspace-main\"]"));
        assert!(THEME_CSS.contains("html.dark [data-pane-id=\"conversation-column\"]"));
        assert!(THEME_CSS.contains("html.dark .side-pane-open-tab-shell"));
        assert!(THEME_CSS.contains(
            "html.dark [data-pane-id]:not([data-pane-id=\"workspace-main\"]):not([data-pane-id=\"conversation-column\"]):not([data-pane-id=\"conversation\"]):not([data-pane-id=\"terminal\"])"
        ));
        // 每条元素规则只消费各自滑块变量并带 !important，防串味：
        // 侧栏容器规则只认侧栏滑块变量、对话区四面板组规则只认对话区
        // 滑块变量、右栏规则只认右栏滑块变量，互相不得混入其他滑块变量
        for (sel, own, others) in [
            (
                "#sidebar",
                "var(--zbar-sidebar-opacity, 0)",
                ["--zbar-panel-opacity", "--zbar-sidebar-right-opacity"],
            ),
            (
                "terminal\"] {",
                "var(--zbar-panel-opacity, 0)",
                ["--zbar-sidebar-opacity", "--zbar-sidebar-right-opacity"],
            ),
            (
                ".side-pane-open-tab-shell {",
                "var(--zbar-sidebar-right-opacity, 0)",
                ["--zbar-panel-opacity", "--zbar-sidebar-opacity"],
            ),
        ] {
            let mut rest = THEME_CSS;
            for _ in 0..2 {
                let start = rest.find(sel).unwrap();
                let tail = &rest[start..];
                let value = &tail[..tail.find(';').unwrap()];
                assert!(value.contains(own), "{sel} 规则应由自身滑块驱动：{value}");
                for other in others {
                    assert!(
                        !value.contains(other),
                        "{sel} 规则不得消费其他滑块变量：{value}"
                    );
                }
                assert!(
                    value.contains("!important"),
                    "{sel} 规则应带 !important 压过源样式：{value}"
                );
                rest = &tail[value.len()..];
            }
        }

        // V10 文字描边：浅深两套各一条容器级规则（描边由文字继承，
        // 选择器组与 V9 主区域背景规则同批），强度全部消费
        // --zbar-text-shadow（variables.css 渲染真值 + 热重载；兜底
        // 0 仅作旧 variables.css 容错，0=关闭时 alpha 为 0 不可见）
        assert_eq!(
            THEME_CSS.matches("text-shadow:").count(),
            2,
            "文字描边规则应在浅色与深色各一条"
        );
        assert_eq!(
            THEME_CSS.matches("var(--zbar-text-shadow").count(),
            4,
            "每条描边规则的模糊半径与 alpha 各消费一次强度变量"
        );
        // 深色黑描边 / 浅色白描边对称，模糊半径随强度温和缩放
        assert!(
            THEME_CSS.contains("rgba(0, 0, 0, var(--zbar-text-shadow, 0))"),
            "深色主题应为黑色描边"
        );
        assert!(
            THEME_CSS.contains("rgba(255, 255, 255, var(--zbar-text-shadow, 0))"),
            "浅色主题应为白色描边"
        );
        assert_eq!(
            THEME_CSS.matches("calc(3px + (var(--zbar-text-shadow, 0) * 5px))").count(),
            2,
            "模糊半径应随强度缩放且浅深各消费一次"
        );
        // 描边规则必须位于全部 V9 规则之后（文件末尾追加，V9 规则
        // 未被改写移动）：V9 最后一条规则（深色右栏背景）先于 V10 注释头
        let v9_last = THEME_CSS
            .find("html.dark .side-pane-open-tab-shell {\n  background-color:")
            .expect("V9 深色右栏背景规则应存在");
        let v10_block = THEME_CSS
            .find("文字可读性：壁纸过亮/过暗时给前景文字补描边")
            .expect("V10 描边注释头应存在");
        assert!(
            v9_last < v10_block,
            "V10 规则只能追加在 V9 规则之后，不得改写或移动既有规则"
        );

        // 热重载核心逻辑特征：data 标记定位 + href 兜底匹配 + 时间戳重读
        assert!(EFFECTS_JS.contains("link[data-zbar-variables]"));
        assert!(EFFECTS_JS.contains("link[href*=\"variables.css\"]"));
        assert!(EFFECTS_JS.contains("?t=\" + Date.now()"));
        // V5 撤销 theme.css 每秒 cache-bust：样式表 href 变更存在
        // "卸载失效 → 异步加载 → 恢复"窗口，失效窗口内三区域背景/文字
        // 描边规则整体失效，背景闪回原生底色形成周期闪烁；theme.css
        // 模板升级改由面板"重启 ZCode"冷启动完全重载
        assert!(
            !EFFECTS_JS.contains("reloadThemeLink") && !EFFECTS_JS.contains("findThemeLink"),
            "theme.css 热重载函数应已整体删除"
        );
        // poll 竞态防御：先快照后重读（避免快照撞上本轮重载失效窗口）+
        // 空值防御（任一变量读到空串视为失效窗口，本轮直接返回）
        assert!(
            EFFECTS_JS.contains("视为失效窗口"),
            "poll 空值防御注释应存在"
        );
        assert!(
            EFFECTS_JS.contains("now[VAR_NAMES[i]] === \"\""),
            "poll 变量空串检查应存在"
        );
        // 先快照后重读的顺序：reloadVarsLink() 调用（仅 poll 中一处）
        // 必须位于 snapshotOf() 快照语句之后
        assert!(
            EFFECTS_JS.find("var now = snapshotOf();")
                < EFFECTS_JS.find("reloadVarsLink();"),
            "poll 应先取快照再重读 variables.css"
        );
        // 换源淡入（可重复调用）与遮罩层
        assert!(EFFECTS_JS.contains("transition:opacity"));
        assert!(EFFECTS_JS.contains("rgba(0,0,0,"));

        // V3 图片支持特征：按扩展名分派 video/img 元素、img 同层样式、
        // 类型切换重建、播放速率仅视频
        assert!(EFFECTS_JS.contains("function kindOf(url)"));
        assert!(EFFECTS_JS.contains("createImage"));
        assert!(EFFECTS_JS.contains("createElement(\"img\")"));
        assert!(EFFECTS_JS.contains("mediaKind !== kind"));
        assert!(EFFECTS_JS.contains("mediaKind !== \"video\""));
        // img 的 onload 淡入语义
        assert!(EFFECTS_JS.contains("i.addEventListener(\"load\", onReady)"));
    }

    #[test]
    fn 注入_link带data标记() {
        let dir = test_dir("data-attrs");
        let index = dir.join("index.html");
        fs::write(&index, SAMPLE_HTML).unwrap();

        let html = apply_inject(&index, &dir).unwrap();
        // effects.js 热重载定位 variables.css 依赖 data-zbar-variables
        assert!(html.contains("data-zbar-variables"), "应注入 data-zbar-variables: {html}");
        assert!(html.contains("data-zbar-theme"));
        assert!(html.contains("data-zbar-effects"));
        // usage.js（用量统计条）注入行带 data-zbar-usage 标记
        assert!(html.contains("data-zbar-usage"), "应注入 data-zbar-usage: {html}");

        // 标记都在注入注释块内：剥离逻辑不受影响，还原后无残留
        assert_eq!(strip_inject_blocks(&html), SAMPLE_HTML);

        let _ = fs::remove_dir_all(&dir);
    }
}

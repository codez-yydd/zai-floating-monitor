//! asar 注入物：主题模板（全部自研）、注入块幂等插入与 variables.css 生成。
//!
//! 注入方式：在 ZCode 的 asar 内 out/renderer/index.html 中，
//! `</head>` 前插入两个外链样式（variables.css + theme.css）、
//! `</body>` 前插入 defer 脚本（effects.js），全部指向 ~/.zbar/agent-themes/
//! 下的主题文件（file:// URL，不改动应用其它资源）。
//! 注入的外链带 data-zbar-variables / data-zbar-theme / data-zbar-effects
//! 标记：effects.js 热重载靠 data-zbar-variables 定位 variables.css
//! （旧版注入行无 data 属性时回退按 href 匹配，同样可热重载）。
//! 整段引用包裹在 <!--ZBAR-THEME-BEGIN--> … <!--ZBAR-THEME-END--> 标记内，
//! 重复安装时先剥离旧标记块再插入新块，保证幂等。

use crate::agent_theme::store::{BASE_ALPHA, ThemeParams};
use std::fs;
use std::path::Path;

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
 * ZBAR-THEME-V9
 * ZBar Agent 动态壁纸主题样式（由 ZBar 落盘并随版本升级覆盖）
 * ============================================================
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
 * TODO: 若调大遮罩仍不满足，可在此追加与 --zbar-mask-strength
 * 联动的 text-shadow。 */
"#;

/// 壁纸运行时脚本模板：读取 --zbar-* CSS 变量，在 body 上挂黑底占位层、
/// 壁纸媒体层（视频或图片，按壁纸扩展名二选一）与压暗遮罩层，并每秒
/// 热重载 variables.css——ZBar 面板改参数/换壁纸无需重启 ZCode 即时生效。
/// 版本化落盘（头部 ZBAR-THEME-V 标记，见 store::ensure_versioned_template）。
pub const EFFECTS_JS: &str = r#"// ============================================================
// ZBAR-THEME-V3
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
      reloadVarsLink();
      var now = snapshotOf();
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

/// 绝对路径 → file:// URL（Unix 形如 file:///Users/…，Windows 形如 file:///C:/…）
pub fn file_url(path: &Path) -> String {
    #[cfg(windows)]
    {
        let unified = path.to_string_lossy().replace('\\', "/");
        format!("file:///{}", percent_encode_path(&unified))
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
/// --zbar-base-alpha 为固定氛围透明度（BASE_ALPHA，非用户参数）：
/// theme.css V5 起全部全局底色 token 由它驱动，与滑块解绑。
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
         \x20 --zbar-base-alpha: {base};\n\
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
        base = BASE_ALPHA,
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
///   - effects.js（defer，带 data-zbar-effects 标记）于 `</body>` 前
/// 返回写回后的完整 html。
pub fn apply_inject(staging_index_html: &Path, theme_dir: &Path) -> Result<String, String> {
    let raw = fs::read_to_string(staging_index_html)
        .map_err(|e| format!("读取 index.html 失败: {e}"))?;
    let cleaned = strip_inject_blocks(&raw);

    let vars_url = file_url(&theme_dir.join(crate::agent_theme::store::VARIABLES_CSS));
    let theme_url = file_url(&theme_dir.join(crate::agent_theme::store::THEME_CSS));
    let effects_url = file_url(&theme_dir.join(crate::agent_theme::store::EFFECTS_JS));

    let head_block = format!(
        "{INJECT_BEGIN}\n<link rel=\"stylesheet\" href=\"{vars_url}\" data-zbar-variables=\"\">\n<link rel=\"stylesheet\" href=\"{theme_url}\" data-zbar-theme=\"\">\n{INJECT_END}\n"
    );
    let body_block = format!(
        "{INJECT_BEGIN}\n<script defer src=\"{effects_url}\" data-zbar-effects=\"\"></script>\n{INJECT_END}\n"
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

        // 引用位置：样式在 </head> 之前、脚本在 </body> 之前
        let head_pos = html2.to_ascii_lowercase().find("</head>").unwrap();
        let vars_pos = html2.find("variables.css").unwrap();
        assert!(vars_pos < head_pos, "样式链接应位于 </head> 之前");
        let body_pos = html2.to_ascii_lowercase().find("</body>").unwrap();
        let js_pos = html2.find("effects.js").unwrap();
        assert!(js_pos < body_pos, "脚本应位于 </body> 之前（defer）");
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
            "--zbar-playback-rate",
        ] {
            assert!(css.contains(var), "variables.css 缺少变量 {var}");
        }
        // 壁纸地址以 url("…") 形式写入
        assert!(css.contains(&format!("url(\"{url}\")")));
        // 默认参数值渲染（V6 默认：亮度/饱和度拉满、对话列/侧栏/右栏
        // 归零、固定氛围值 0.25、速率 1）
        assert!(css.contains("--zbar-wp-brightness: 1.1;"));
        assert!(css.contains("--zbar-wp-saturate: 1.4;"));
        assert!(css.contains("--zbar-wp-blur: 0px;"));
        assert!(css.contains("--zbar-mask-strength: 0;"));
        assert!(css.contains("--zbar-panel-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-opacity: 0;"));
        assert!(css.contains("--zbar-sidebar-right-opacity: 0;"));
        assert!(css.contains("--zbar-base-alpha: 0.25;"));
        assert!(css.contains("--zbar-playback-rate: 1;"));
        // 非 ASCII 壁纸名在 url 里必须已编码（不出现裸中文）
        assert!(!css.contains("我的壁纸"));
    }

    #[test]
    fn has_inject_判定() {
        assert!(!has_inject(SAMPLE_HTML));
        assert!(has_inject(&format!("x{INJECT_BEGIN}y{INJECT_END}z")));
    }

    #[test]
    fn 模板_版本头与token覆盖与图片支持() {
        // 头部版本标记（store::ensure_versioned_template 的升级判据）：
        // theme.css 升 V9（运行时实测选择器终版修正），
        // effects.js V3（图片壁纸支持）
        assert!(THEME_CSS.contains("ZBAR-THEME-V9"));
        assert!(EFFECTS_JS.contains("ZBAR-THEME-V3"));

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

        // V9 主区域元素级规则：侧栏容器在浅色、深色各一条；对话区
        // 四面板组（常驻 workspace-main + 多面板视图的对话列三面板
        // 组）在浅色、深色各一条；右栏其余面板（:not 链排除四面板）
        // 与右栏空态选择面板容器类在浅色、深色各一条（html.dark 前
        // 缀形态也含选择器子串，计数即规则数；注释中不出现选择器
        // 字面量，不影响计数）
        assert_eq!(
            THEME_CSS.matches("#sidebar").count(),
            2,
            "侧栏容器元素规则应在浅色与深色各一条"
        );
        // V6 修正：#content 实为"对话列 + 右侧面板"整体容器，其元素规则
        // 已删除，对话区滑块不再作用其上
        assert_eq!(
            THEME_CSS.matches("#content").count(),
            0,
            "#content 元素规则应已移除（否则会牵连右侧面板）"
        );
        // V9 对话区四面板组：四个面板值各出现 4 次——对话区规则浅深
        // 各 1 次 + 右栏 :not 链浅深各 1 次（缺一即归属串味）
        for pane in [
            "workspace-main",
            "conversation-column",
            "conversation",
            "terminal",
        ] {
            let needle = format!("{pane}\"]");
            assert_eq!(
                THEME_CSS.matches(&needle).count(),
                4,
                "面板值 {pane} 应在对话区规则与右栏 :not 链中各出现浅深 2 次"
            );
        }
        // 对话区规则与右栏规则以各自选择器末段形态区分：后跟 " {" 的
        // terminal 选择器是对话区组末段，右栏组末段是空态选择面板
        // 容器类（后跟 " {"）
        assert_eq!(
            THEME_CSS.matches("terminal\"] {").count(),
            2,
            "对话区四面板组元素规则应在浅色与深色各一条"
        );
        assert_eq!(
            THEME_CSS.matches(".side-pane-open-tab-shell {").count(),
            2,
            "右栏元素规则（:not 链 + 空态选择面板容器类）应在浅色与深色各一条"
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
            18,
            "对话区 2 条规则各 4 次（四面板组）+ 右栏 2 条规则各 5 次（本体 + 四段 :not）"
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

        // 热重载核心逻辑特征：data 标记定位 + href 兜底匹配 + 时间戳重读
        assert!(EFFECTS_JS.contains("link[data-zbar-variables]"));
        assert!(EFFECTS_JS.contains("link[href*=\"variables.css\"]"));
        assert!(EFFECTS_JS.contains("?t=\" + Date.now()"));
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

        // 标记都在注入注释块内：剥离逻辑不受影响，还原后无残留
        assert_eq!(strip_inject_blocks(&html), SAMPLE_HTML);

        let _ = fs::remove_dir_all(&dir);
    }
}

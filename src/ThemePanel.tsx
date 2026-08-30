import { useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { openUrl } from "@tauri-apps/plugin-opener";
import { convertFileSrc } from "@tauri-apps/api/core";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  getAgentThemeParams,
  getAgentThemeState,
  getPetConfig,
  installAgentTheme,
  listAgentWallpapers,
  restartZcode,
  selectAgentWallpaper,
  setAgentThemeParams,
  setAgentWallpaper,
  setAgentWallpaperDir,
  setPanelSticky,
  setPetConfig,
  uninstallAgentTheme,
} from "./api";
import {
  AlertBanner,
  BtnPrimary,
  BtnSecondary,
  LoadingState,
  PageBody,
  PageHeader,
  PageShell,
  PillButton,
  PillGroup,
  SettingsCard,
  StatusBadge,
} from "./layout";
import { useI18n, type MessageKey } from "./i18n";
import {
  PET_IMPORT_FILE_RE,
  PET_IMPORT_IMAGE_RE,
  PetSizeLevelPicker,
  PetStyleSection,
  useCustomPets,
} from "./petStyles";
import type {
  AgentThemeProgress,
  AgentThemeState,
  PetConfig,
  PetMode,
  ThemeParams,
  WallpaperEntry,
} from "./types";

/** MVP 支持的 Agent 应用（Rust 侧按 appId 分发；后续新增应用在此登记） */
const AGENTS: ReadonlyArray<{ appId: string; name: string }> = [
  { appId: "zcode", name: "ZCode" },
];

/** ZCode 官网（macOS 确认浮层提示中的重新下载入口） */
const ZCODE_OFFICIAL_SITE = "https://zcode.z.ai";

/** 是否 macOS（与 StatsPanel 的 isWindows 同款判断）：确认浮层的更新影响提示仅 macOS 展示 */
const isMac =
  typeof navigator !== "undefined" && /mac/i.test(navigator.userAgent);

/** Rust 侧安装/还原进度事件名 */
const PROGRESS_EVENT = "zbar://agent-theme-progress";

/** 进度 stage 值 → 词典键（未收录的 stage 原样展示，后端新增阶段不丢信息） */
const STAGE_KEYS: Record<string, MessageKey> = {
  precheck: "theme.stage.precheck",
  quit: "theme.stage.quit",
  extract: "theme.stage.extract",
  inject: "theme.stage.inject",
  pack: "theme.stage.pack",
  verify: "theme.stage.verify",
  backup: "theme.stage.backup",
  replace: "theme.stage.replace",
  sign: "theme.stage.sign",
  launch: "theme.stage.launch",
  cleanup: "theme.stage.cleanup",
  done: "theme.stage.done",
  error: "theme.stage.error",
};

/**
 * 数值滑块定义：min/max/step 为滑块刻度，format 接收刻度值。
 *
 * 刻度与 Rust 侧存储值的换算（关键约定）：
 * - 带 `scale` 的滑块：Rust 侧 ThemeParams 存小数（如亮度 0.4~1.1），
 *   滑块用百分比刻度（40~110），存值时 ÷scale、展示时 ×scale（见 toScale/fromScale）
 * - 不带 `scale` 的滑块（wpBlur 的 px / playbackRate 的倍速）：刻度即存储值，直存
 */
interface SliderDef {
  key: keyof Omit<
    ThemeParams,
    "wallpaperFile" | "wallpaperDir" | "usageSessionBar" | "usageTurnBar"
  >;
  labelKey: MessageKey;
  /** 可选滑块说明（渲染在滑块下方的小字；仅部分参数提供） */
  hintKey?: MessageKey;
  min: number;
  max: number;
  step: number;
  /** 刻度→存储值的换算系数（100 = 百分比刻度，存储值 = 刻度值/100）；缺省 = 刻度即存储值 */
  scale?: number;
  format: (v: number) => string;
}

/** 壁纸效果参数滑块（"效果参数"卡片内平铺渲染，见下方卡片） */
const SLIDERS: ReadonlyArray<SliderDef> = [
  {
    key: "wpBrightness",
    labelKey: "theme.paramWpBrightness",
    min: 40,
    max: 110,
    step: 1,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "wpSaturate",
    labelKey: "theme.paramWpSaturate",
    min: 40,
    max: 140,
    step: 1,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "wpBlur",
    labelKey: "theme.paramWpBlur",
    min: 0,
    max: 20,
    step: 0.5,
    format: (v) => `${v}px`,
  },
  {
    // 氛围底（可读性增强）：壁纸之上的主题色半透明底，存储 0~1、步进 0.05，
    // 换算为百分比刻度 0~100、步进 5
    key: "baseAlpha",
    labelKey: "theme.paramBaseAlpha",
    hintKey: "theme.paramBaseAlphaHint",
    min: 0,
    max: 100,
    step: 5,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "maskStrength",
    labelKey: "theme.paramMaskStrength",
    min: 0,
    max: 90,
    step: 1,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    // 文字描边（可读性增强）：存储 0~1、步进 0.05，换算为百分比刻度
    // 0~100、步进 5（0 = 关闭）
    key: "textShadow",
    labelKey: "theme.paramTextShadow",
    hintKey: "theme.paramTextShadowHint",
    min: 0,
    max: 100,
    step: 5,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "panelOpacity",
    labelKey: "theme.paramPanelOpacity",
    min: 0,
    max: 100,
    step: 1,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "sidebarOpacity",
    labelKey: "theme.paramSidebarOpacity",
    min: 0,
    max: 100,
    step: 1,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    // 右栏独立滑块（V6）：存储 0~1、步进 0.05，换算为百分比刻度
    // 0~100、步进 5（scale 机制：存储值 = 刻度值/100）
    key: "sidebarRightOpacity",
    labelKey: "theme.paramSidebarRightOpacity",
    min: 0,
    max: 100,
    step: 5,
    scale: 100,
    format: (v) => `${v}%`,
  },
  {
    key: "playbackRate",
    labelKey: "theme.paramPlaybackRate",
    min: 0.5,
    max: 2,
    step: 0.05,
    format: (v) => `${v.toFixed(2)}×`,
  },
];

/**
 * 用量统计条滑块定义：独立于壁纸效果参数的单独配置区域（独立 SettingsCard
 * 渲染，卡片结构见下方"用量统计条"区），调整 ZCode 对话内每轮末尾用量
 * 统计条的字号与文字不透明度，同样经 variables.css 热重载即时生效。
 * - usageFontSize：整数 px 刻度（9~16，步进 1），刻度即存储值直存
 * - usageOpacity：存储 0.25~1 小数、百分比刻度 25~100、步进 5
 *   （scale 机制同 SLIDERS：存储值 = 刻度值/100）
 */
const USAGE_SLIDERS: ReadonlyArray<SliderDef> = [
  {
    key: "usageFontSize",
    labelKey: "theme.paramUsageFontSize",
    hintKey: "theme.paramUsageFontSizeHint",
    min: 9,
    max: 16,
    step: 1,
    format: (v) => `${v}px`,
  },
  {
    key: "usageOpacity",
    labelKey: "theme.paramUsageOpacity",
    hintKey: "theme.paramUsageOpacityHint",
    min: 25,
    max: 100,
    step: 5,
    scale: 100,
    format: (v) => `${v}%`,
  },
];

/**
 * 桌面宠物大小：屏高比例档位 1~5（5.5%~15%），不用滑杆——渲染时 Rust
 * 侧按主显示器逻辑高换算成整数 px 写入 --zbar-pet-size（pet.js 画布
 * CSS 尺寸直接消费，换机器观感一致），UI 用 petStyles 的
 * PetSizeLevelPicker 分段控件（与字号档位同风格）；档位存 PetConfig
 * （pet.json），改完经 setPetConfig 即时生效。
 */

/** Rust 存储值 → 滑块刻度值（scale 滑块 ×scale，并消除浮点尾差如 0.85×100） */
const toScale = (v: number, scale?: number) =>
  scale ? Math.round(v * scale * 1000) / 1000 : v;

/** 滑块刻度值 → Rust 存储值（scale 滑块 ÷scale，如 85 → 0.85） */
const fromScale = (v: number, scale?: number) => (scale ? v / scale : v);

/**
 * 效果参数出厂默认（与 Rust 侧 ThemeParams::default 一致；"恢复默认参数"
 * 按钮与新增安装的初始观感都以此为准：亮度/饱和度拉满、无模糊遮罩、
 * 氛围底保持基础垫底（0.25）且文字描边关闭、面板与侧栏全透明（V5 分层
 * 后滑块各管各的容器、互不牵连；V6 起右栏亦有独立滑块，同样默认全透明；
 * 其余区域由固定氛围透明度兜底）、原速播放、用量统计条 10px / 55% 与
 * 注入模板原写死观感一致）
 */
const DEFAULT_EFFECT_PARAMS: Pick<
  ThemeParams,
  | "wpBrightness"
  | "wpSaturate"
  | "wpBlur"
  | "baseAlpha"
  | "maskStrength"
  | "textShadow"
  | "panelOpacity"
  | "sidebarOpacity"
  | "sidebarRightOpacity"
  | "playbackRate"
  | "usageFontSize"
  | "usageOpacity"
> = {
  wpBrightness: 1.1,
  wpSaturate: 1.4,
  wpBlur: 0,
  baseAlpha: 0.25,
  maskStrength: 0,
  textShadow: 0,
  panelOpacity: 0,
  sidebarOpacity: 0,
  sidebarRightOpacity: 0,
  playbackRate: 1,
  usageFontSize: 10,
  usageOpacity: 0.55,
};

/**
 * 亮色壁纸适配预设：亮色壁纸下暗色主题文字可读性的推荐参数组合。
 * 只覆盖这四项（氛围底/遮罩/壁纸亮度/文字描边），其余参数保持用户当前值。
 */
const LIGHT_WALLPAPER_PRESET: Pick<
  ThemeParams,
  "baseAlpha" | "maskStrength" | "wpBrightness" | "textShadow"
> = {
  baseAlpha: 0.55,
  maskStrength: 0.35,
  wpBrightness: 0.6,
  textShadow: 0.6,
};

/** 拖放导入的壁纸文件白名单（与 Rust 侧扩展名白名单一致，大小写不敏感） */
const WALLPAPER_FILE_RE = /\.(mp4|webm|mov|jpe?g|png|webp)$/i;

/** 兼容 Windows 分隔符取路径的最后一段 */
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/**
 * 壁纸库"当前使用项"判定：wallpaperFile 有三种存值形态——
 * "default.mp4"（默认）、相对文件名（拖拽导入历史）、绝对路径（壁纸库选择），
 * 与条目的 path / fileName 双口径比对以兼容全部形态。
 */
const isCurrentEntry = (
  entry: WallpaperEntry,
  current: string | null | undefined
): boolean => {
  const cur = current?.trim();
  if (entry.path === "default") {
    return !cur || cur === "default.mp4";
  }
  return !!cur && (cur === entry.path || cur === entry.fileName);
};

/** 需要二次确认浮层的危险动作 */
type ConfirmAction = "install" | "uninstall";

interface Props {
  onBack: () => void;
}

/**
 * 动态壁纸页：遥控器侧的安装 / 还原 / 效果参数面板。
 *
 * - 挂载与每次安装/还原完成后刷新 get_agent_theme_state（首屏有加载态，
 *   后续刷新静默进行，以 Rust 侧状态为准）
 * - 安装/还原由 Rust 侧经 zbar://agent-theme-progress 事件推送分阶段进度
 * - 滑块本地即时反馈，300ms 防抖整体落盘 set_agent_theme_params
 */
export function ThemePanel({ onBack }: Props) {
  const { t } = useI18n();
  // null = 首屏状态读取中；读取失败保持 null 并经 AlertBanner 提示
  const [state, setState] = useState<AgentThemeState | null>(null);
  const [params, setParams] = useState<ThemeParams | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  // install / uninstall 进行中（进度条展示 + 全部按钮禁用）
  const [busy, setBusy] = useState<ConfirmAction | null>(null);
  const [progress, setProgress] = useState<AgentThemeProgress | null>(null);
  const [confirm, setConfirm] = useState<ConfirmAction | null>(null);
  // 重启 ZCode：二次确认展示中 / 执行中（执行时按钮禁用并切换文案）
  const [confirmRestart, setConfirmRestart] = useState(false);
  const [restarting, setRestarting] = useState(false);
  const [changingWallpaper, setChangingWallpaper] = useState(false);
  // 壁纸库：列表数据与加载/切换/设目录的中间态
  const [wallpapers, setWallpapers] = useState<WallpaperEntry[] | null>(null);
  const [wallpapersLoading, setWallpapersLoading] = useState(false);
  const [selectingPath, setSelectingPath] = useState<string | null>(null);
  const [settingDir, setSettingDir] = useState(false);
  // 拖放悬停高亮（enter/over 置真，leave/drop 复位）
  const [dragOver, setDragOver] = useState(false);
  const [paramsSavedFlash, setParamsSavedFlash] = useState(false);
  // 参数保存防抖 timer + 最新参数镜像（回调闭包读不到最新 state，从 ref 取）
  const saveTimer = useRef<number | undefined>(undefined);
  const paramsRef = useRef<ThemeParams | null>(null);
  paramsRef.current = params;
  // 成功反馈自动消失 timer
  const flashTimer = useRef<number | undefined>(undefined);
  // 拖放事件回调闭包读不到最新 state，用 ref 镜像守卫：
  // active = 已安装且无需重装时才接受投放；locked = 处理中防重入
  const dragGuardRef = useRef({ active: false, locked: false });
  // onDragDropEvent 的 effect 闭包只注册一次，经 ref 转发到最新处理函数
  const dropHandlerRef = useRef<(paths: string[]) => void>(() => {});

  // ===== 桌面宠物（pet.json/PetConfig 唯一真相源：总开关 + 注入版/
  // 悬浮窗形态二选一 + 形象/尺寸，改完 setPetConfig 即时生效）=====
  const [petCfg, setPetCfg] = useState<PetConfig | null>(null);
  // 最新宠物配置镜像（拖放兜底等一次性闭包读不到最新 state）
  const petCfgRef = useRef<PetConfig | null>(null);

  // 自定义宠物（第三阶段）：清单/拖放导入/删除共享控制器；导入或删除
  // 后重拉宠物配置（删除正在使用的宠物时 Rust 侧会把选中回退内建默认
  // 形象并热生效，面板需要重新读取才能同步高亮）
  const refreshPetCfgQuiet = () => {
    getPetConfig()
      .then((c) => {
        petCfgRef.current = c;
        setPetCfg(c);
      })
      .catch(() => {});
  };
  const customPets = useCustomPets(refreshPetCfgQuiet);

  /** 刷新壁纸库列表（打开面板 / 设目录 / 导入后调用） */
  const refreshWallpapers = () => {
    setWallpapersLoading(true);
    listAgentWallpapers("zcode")
      .then(setWallpapers)
      .catch((e) =>
        setError(t("theme.listWallpapersFail", { msg: String(e) }))
      )
      .finally(() => setWallpapersLoading(false));
  };

  /**
   * 刷新安装状态；壁纸库列表请求与状态查询并行发出（原先等状态返回
   * 后才拉列表，已安装态首屏多等一程）。列表结果仅在确认已安装后写入
   * state（未安装时清空），渲染仍由 state.installed 门控，错误处理照旧。
   */
  const refreshState = () => {
    setWallpapersLoading(true);
    const listPromise = listAgentWallpapers("zcode");
    getAgentThemeState("zcode")
      .then((s) => {
        setState(s);
        if (!s.installed) {
          setParams(null);
          setWallpapers(null);
          setWallpapersLoading(false);
          return;
        }
        getAgentThemeParams("zcode")
          .then(setParams)
          .catch((e) =>
            setError(t("theme.loadParamsFail", { msg: String(e) }))
          );
        listPromise
          .then(setWallpapers)
          .catch((e) =>
            setError(t("theme.listWallpapersFail", { msg: String(e) }))
          )
          .finally(() => setWallpapersLoading(false));
      })
      .catch((e) => {
        setError(t("theme.loadStateFail", { msg: String(e) }));
        setWallpapersLoading(false);
      });
  };

  // 挂载读取首屏状态 + 订阅安装/还原进度事件
  useEffect(() => {
    // 开启面板粘滞：皮肤页打开期间失焦不自动隐藏（拖文件/看进度/处理
    // 授权弹窗时需切窗）。fire-and-forget，失败不影响页面其余功能。
    setPanelSticky(true).catch(() => {});
    refreshState();
    let unlisten: (() => void) | undefined;
    let disposed = false;
    listen<AgentThemeProgress>(PROGRESS_EVENT, (ev) => {
      if (ev.payload.appId !== "zcode") return;
      setProgress(ev.payload);
    })
      .then((fn) => {
        // 订阅完成前组件已卸载：立即注销，避免泄漏
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 订阅失败仅退化为无进度条，安装结果仍由 invoke 返回 */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 宠物配置加载（挂载即拉取；失败不阻塞页面其余功能，宠物卡停在本地
  // 面板不渲染——悬浮窗形态不依赖皮肤安装，未安装皮肤也可设置，选注入
  // 版时装完皮肤即出现）
  useEffect(() => {
    getPetConfig()
      .then((c) => {
        petCfgRef.current = c;
        setPetCfg(c);
      })
      .catch((e) => setError(t("theme.petLoadFail", { msg: String(e) })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 卸载清理：冲掉未触发的参数保存防抖与反馈清除 timer
  useEffect(() => {
    return () => {
      // 关闭皮肤页必须恢复默认失焦隐藏，避免粘滞标志泄漏
      setPanelSticky(false).catch(() => {});
      if (saveTimer.current !== undefined) {
        window.clearTimeout(saveTimer.current);
      }
      if (flashTimer.current !== undefined) {
        window.clearTimeout(flashTimer.current);
      }
    };
  }, []);

  // 挂载订阅 Tauri 拖放事件（换壁纸入口）；卸载注销，参照上方 listen 的 disposed 模式。
  // enter/over 高亮投放区，leave 取消高亮，drop 取首个文件走换壁纸流程
  useEffect(() => {
    let unlisten: (() => void) | undefined;
    let disposed = false;
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "enter" || event.payload.type === "over") {
          setDragOver(true);
        } else if (event.payload.type === "leave") {
          setDragOver(false);
        } else if (event.payload.type === "drop") {
          setDragOver(false);
          dropHandlerRef.current(event.payload.paths);
        }
      })
      .then((fn) => {
        // 订阅完成前组件已卸载：立即注销，避免泄漏
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 订阅失败仅退化为无拖放入口，不影响安装/参数等其余功能 */
      });
    return () => {
      disposed = true;
      unlisten?.();
    };
  }, []);

  /** 轻量成功反馈：顶部绿色横幅，4s 后自动消失 */
  const showFlash = (text: string) => {
    setFlash(text);
    if (flashTimer.current !== undefined) {
      window.clearTimeout(flashTimer.current);
    }
    flashTimer.current = window.setTimeout(() => setFlash(null), 4000);
  };

  /** 执行安装/还原（确认浮层通过后调用），进度经事件驱动，结束后刷新状态 */
  const runAction = async (action: ConfirmAction) => {
    setConfirm(null);
    setError(null);
    setFlash(null);
    setBusy(action);
    setProgress(null);
    try {
      if (action === "install") {
        await installAgentTheme("zcode");
        showFlash(t("theme.installDone"));
      } else {
        await uninstallAgentTheme("zcode");
        showFlash(t("theme.uninstallDone"));
      }
    } catch (e) {
      setError(
        t(action === "install" ? "theme.installFail" : "theme.uninstallFail", {
          msg: String(e),
        })
      );
    } finally {
      setBusy(null);
      setProgress(null);
      // 成败都以 Rust 侧状态为准刷新（失败时可能处于中间状态）
      refreshState();
    }
  };

  /**
   * 参数防抖落盘：300ms 防抖后把完整参数整体落盘（滑块与开关共用管道），
   * 成功时顶部闪现"已保存"反馈，失败进 AlertBanner。
   */
  const scheduleParamsSave = (next: ThemeParams) => {
    if (saveTimer.current !== undefined) {
      window.clearTimeout(saveTimer.current);
    }
    saveTimer.current = window.setTimeout(() => {
      saveTimer.current = undefined;
      setAgentThemeParams("zcode", next)
        .then(() => {
          setParamsSavedFlash(true);
          window.setTimeout(() => setParamsSavedFlash(false), 1500);
        })
        .catch((e) =>
          setError(t("theme.setParamsFail", { msg: String(e) }))
        );
    }, 300);
  };

  /**
   * 滑块变更：本地即时反馈，300ms 防抖后把完整参数整体落盘。
   * `value` 为滑块刻度值，scale 滑块先经 fromScale 换算回 Rust 存储小数再存
   * （如亮度刻度 85 → 存 0.85），否则会被 Rust 侧 clamp 到 0.4~1.1 破坏数值。
   */
  const handleSlider = (
    key: keyof Omit<ThemeParams, "wallpaperFile">,
    value: number,
    scale?: number
  ) => {
    const cur = paramsRef.current;
    if (!cur) return;
    const next = { ...cur, [key]: fromScale(value, scale) };
    setParams(next);
    scheduleParamsSave(next);
  };

  /**
   * 会话累计条开关变更（布尔参数，不走滑块刻度换算）：本地即时反馈，
   * 防抖保存管道与滑块共用；Rust 侧落盘后经 variables.css 的
   * --zbar-usage-session-bar 热重载透传给注入侧 usage.js（约 1 秒生效）。
   */
  const handleUsageSessionBar = (checked: boolean) => {
    const cur = paramsRef.current;
    if (!cur) return;
    const next = { ...cur, usageSessionBar: checked };
    setParams(next);
    scheduleParamsSave(next);
  };

  /**
   * 每轮统计条开关变更（布尔参数，不走滑块刻度换算）：本地即时反馈，
   * 防抖保存管道与滑块共用；Rust 侧落盘后经 variables.css 的
   * --zbar-usage-turn-bar 热重载透传给注入侧 usage.js（约 1 秒生效）。
   */
  const handleUsageTurnBar = (checked: boolean) => {
    const cur = paramsRef.current;
    if (!cur) return;
    const next = { ...cur, usageTurnBar: checked };
    setParams(next);
    scheduleParamsSave(next);
  };

  /**
   * 应用宠物配置（pet.json 唯一真相源，set_pet_config 改完即生效，不走
   * 参数防抖管道）：乐观更新 + 失败回读回滚——
   * - 总开关/形态切换：注入版经 variables.css 热重载约 1 秒生效（需已
   *   安装皮肤）；悬浮窗即时建/关窗；
   * - 形象/尺寸变化：注入版热切换重建画布，悬浮窗同步尺寸热推参数。
   */
  const applyPet = async (next: PetConfig) => {
    petCfgRef.current = next;
    setPetCfg(next);
    try {
      const back = await setPetConfig(next);
      petCfgRef.current = back;
      setPetCfg(back);
    } catch (e) {
      setError(t("theme.petApplyFail", { msg: String(e) }));
      try {
        const back = await getPetConfig();
        petCfgRef.current = back;
        setPetCfg(back);
      } catch {
        /* 回读失败保持当前态（下次切换再对齐） */
      }
    }
  };

  /**
   * 处理拖放：原生文件对话框在无 Dock 图标的 Accessory（ZBar）应用上
   * 不可见，且其模态等待会阻塞主线程导致所有 IPC 瘫痪，故改为接收 Tauri
   * 拖放事件给出的文件路径。多文件时只取第一个，按内容分流：
   * - 宠物导入文件（zip / pet.json）→ 自定义宠物导入（Petdex 包，见
   *   petStyles 的 PetStyleSection），不依赖皮肤安装，路由在壁纸守卫
   * （active = 已安装）之前；
   * - 裸图集 png/webp → 皮肤未安装时（壁纸导入本就不可用）路由给宠物
   *   导入（与原设置页拖放语义一致，保留裸图集入口）；已安装时保持
   *   壁纸导入语义不变（避免抢占壁纸主流程）；
   * - 壁纸白名单扩展名（大小写不敏感）→ 导入 wallpapers/ 并切换指向；
   * - 其余（无壁纸扩展名）→ 按文件夹处理，交给 Rust 侧校验真实目录性
   *   后设为壁纸目录（非目录时由后端报出明确错误）
   * 任何异常都必须落到 setError。
   */
  const importPetFromDrop = async (path: string) => {
    dragGuardRef.current = { ...dragGuardRef.current, locked: true };
    try {
      const err = await customPets.importFromPath(path);
      // P2-3：宠物卡未渲染（配置读取失败）时导入结果无展示位，经
      // 全局反馈通道兜底（成功走 flash、失败走 error 条）
      if (petCfgRef.current === null) {
        if (err === null) showFlash(t("theme.petImportDone"));
        else setError(err);
      }
    } finally {
      dragGuardRef.current.locked = false;
    }
  };
  const handleDropWallpaper = async (paths: string[]) => {
    const guard = dragGuardRef.current;
    if (guard.locked || paths.length === 0) return;
    const path = paths[0];
    const fileName = baseName(path);
    if (PET_IMPORT_FILE_RE.test(fileName)) {
      await importPetFromDrop(path);
      return;
    }
    // 皮肤未安装时 png/webp 投放路由给宠物导入（裸图集是 Petdex 导入
    // 支持的三种形态之一，设置页宠物卡删除后这里是唯一裸图集入口；
    // guard.active = false = 未安装/需重装，壁纸导入本就不可用）
    if (!guard.active && PET_IMPORT_IMAGE_RE.test(fileName)) {
      await importPetFromDrop(path);
      return;
    }
    if (!guard.active) return;
    // 同步置位防连续 drop 竞态（state 更新需等下一次渲染）
    dragGuardRef.current = { ...guard, locked: true };
    if (WALLPAPER_FILE_RE.test(fileName)) {
      try {
        setChangingWallpaper(true);
        setError(null);
        const { fileName: applied } = await setAgentWallpaper("zcode", path);
        showFlash(t("theme.wallpaperSet", { name: applied }));
        // 参数区展示的当前壁纸指向随之后端刷新，列表补入新导入项
        getAgentThemeParams("zcode")
          .then(setParams)
          .catch(() => {});
        refreshWallpapers();
      } catch (e) {
        setError(t("theme.setWallpaperFail", { msg: String(e) }));
      } finally {
        dragGuardRef.current.locked = false;
        setChangingWallpaper(false);
      }
      return;
    }
    try {
      setSettingDir(true);
      setError(null);
      await setAgentWallpaperDir("zcode", path);
      showFlash(t("theme.wallpaperDirSet", { path: fileName }));
      // 目录变化后重拉参数（wallpaperDir）与列表（新目录内容）
      getAgentThemeParams("zcode")
        .then(setParams)
        .catch(() => {});
      refreshWallpapers();
    } catch (e) {
      setError(t("theme.setWallpaperDirFail", { msg: String(e) }));
    } finally {
      dragGuardRef.current.locked = false;
      setSettingDir(false);
    }
  };
  dropHandlerRef.current = handleDropWallpaper;

  /**
   * 壁纸库点击切换：select 成功后以 Rust 侧参数为准刷新（wallpaperFile
   * 更新 → 高亮迁移），effects.js 热重载约 1 秒生效，无需提示重启。
   */
  const handleSelectWallpaper = async (entry: WallpaperEntry) => {
    if (busy !== null || selectingPath !== null || settingDir) return;
    if (isCurrentEntry(entry, paramsRef.current?.wallpaperFile)) return; // 已是当前项
    setSelectingPath(entry.path);
    setError(null);
    try {
      await selectAgentWallpaper("zcode", entry.path);
      await getAgentThemeParams("zcode")
        .then(setParams)
        .catch(() => {});
      showFlash(
        t("theme.wallpaperSet", {
          name:
            entry.path === "default"
              ? t("theme.defaultWallpaperName")
              : entry.fileName,
        })
      );
    } catch (e) {
      setError(t("theme.selectWallpaperFail", { msg: String(e) }));
    } finally {
      setSelectingPath(null);
    }
  };

  /** 清除壁纸目录（传空串由 Rust 侧置 None），列表随之回落内置目录 */
  const handleClearWallpaperDir = async () => {
    if (busy !== null || settingDir) return;
    setSettingDir(true);
    setError(null);
    try {
      await setAgentWallpaperDir("zcode", "");
      showFlash(t("theme.wallpaperDirCleared"));
      await getAgentThemeParams("zcode")
        .then(setParams)
        .catch(() => {});
      refreshWallpapers();
    } catch (e) {
      setError(t("theme.setWallpaperDirFail", { msg: String(e) }));
    } finally {
      setSettingDir(false);
    }
  };

  /**
   * 亮色壁纸适配：一键写入亮色壁纸下的推荐参数（只覆盖氛围底/遮罩/壁纸
   * 亮度/文字描边四项，其余保持用户当前值）。保存管道与恢复默认一致：
   * 本地即时反馈 + 落盘 set_agent_theme_params（Rust 侧热重载约 1 秒生效）；
   * 写入前先取消未触发的滑块保存防抖，避免旧值稍后覆盖预设；失败时
   * 回滚为后端实际值。
   */
  const handleLightWallpaperPreset = async () => {
    const cur = paramsRef.current;
    if (!cur || busy !== null) return;
    if (saveTimer.current !== undefined) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = undefined;
    }
    const next: ThemeParams = { ...cur, ...LIGHT_WALLPAPER_PRESET };
    setParams(next);
    setError(null);
    try {
      await setAgentThemeParams("zcode", next);
      setParamsSavedFlash(true);
      window.setTimeout(() => setParamsSavedFlash(false), 1500);
    } catch (e) {
      setError(t("theme.lightWallpaperPresetFail", { msg: String(e) }));
      getAgentThemeParams("zcode")
        .then(setParams)
        .catch(() => {});
    }
  };

  /**
   * 恢复默认参数：把全部效果滑块重置为出厂默认（保留当前壁纸指向与
   * 壁纸目录），本地即时反馈 + 落盘；写入前先取消未触发的滑块保存
   * 防抖，避免拖动旧值稍后覆盖重置结果；失败时回滚为后端实际值。
   */
  const handleResetParams = async () => {
    const cur = paramsRef.current;
    if (!cur || busy !== null) return;
    if (saveTimer.current !== undefined) {
      window.clearTimeout(saveTimer.current);
      saveTimer.current = undefined;
    }
    const next: ThemeParams = { ...cur, ...DEFAULT_EFFECT_PARAMS };
    setParams(next);
    setError(null);
    try {
      await setAgentThemeParams("zcode", next);
      setParamsSavedFlash(true);
      window.setTimeout(() => setParamsSavedFlash(false), 1500);
    } catch (e) {
      setError(t("theme.resetParamsFail", { msg: String(e) }));
      getAgentThemeParams("zcode")
        .then(setParams)
        .catch(() => {});
    }
  };

  /**
   * 重启 ZCode：注入的 theme.css / effects.js 依赖应用冷启动加载，手动改过
   * 注入文件等场景热重载覆盖不到，重启后完全重载。经确认浮层后执行：
   * 未运行时后端直接拉起，运行中退出（含强杀兜底与超时报错）后重新启动；
   * 成功用绿色横幅反馈，失败进 AlertBanner（restart 返回 restarted=false
   * 表示原本未在运行、仅直接启动，按两种结果分别提示）。
   */
  const handleRestartZcode = async () => {
    setConfirmRestart(false);
    if (restarting) return;
    setRestarting(true);
    setError(null);
    setFlash(null);
    try {
      const { restarted } = await restartZcode("zcode");
      showFlash(t(restarted ? "theme.launchDone" : "theme.restartDone"));
    } catch (e) {
      setError(t("theme.restartFail", { msg: String(e) }));
    } finally {
      setRestarting(false);
    }
  };

  /** 确认浮层说明文案：基础风险说明 + 运行中时的自动退出提示 */
  const confirmDesc = (action: ConfirmAction) => {
    const base =
      action === "install"
        ? t("theme.confirmInstallDesc")
        : t("theme.confirmUninstallDesc");
    return state?.targetRunning
      ? `${base}\n\n${t("theme.confirmQuitNote")}`
      : base;
  };

  // 首屏：状态未就绪（加载中或读取失败）时只渲染骨架 + 提示
  if (!state) {
    return (
      <PageShell>
        <PageHeader title={t("theme.title")} onBack={onBack} />
        {error ? (
          <PageBody>
            <AlertBanner>{error}</AlertBanner>
            <div className="flex justify-center pt-2">
              <BtnSecondary onClick={refreshState}>
                {t("common.refresh")}
              </BtnSecondary>
            </div>
          </PageBody>
        ) : (
          <LoadingState text={t("theme.loading")} />
        )}
      </PageShell>
    );
  }

  // 按钮统一禁用：安装/还原进行中 + 重启 ZCode 进行中（进行中全部按钮禁用）
  const actionsDisabled = busy !== null || restarting;
  // Node.js 缺失时注入必然失败：安装/重装入口置灰，横幅引导先装 Node
  const installDisabled = actionsDisabled || !state.nodeAvailable;

  // 每次渲染同步拖放守卫镜像，供 onDragDropEvent 回调（一次性闭包）读取
  dragGuardRef.current = {
    active: state.installed && !state.needsReinstall,
    locked: actionsDisabled || changingWallpaper || settingDir,
  };

  return (
    <div className="relative h-full">
      <PageShell>
        <PageHeader title={t("theme.title")} onBack={onBack} />
        <PageBody className="page-stack">
          {error && <AlertBanner>{error}</AlertBanner>}
          {flash && !busy && (
            <AlertBanner type="success">{flash}</AlertBanner>
          )}

          {/* Agent 应用卡片（map 渲染，MVP 只有 ZCode；状态按单一 state 驱动） */}
          {AGENTS.map((agent) => (
            <SettingsCard
              key={agent.appId}
              title={agent.appId === "zcode" ? t("theme.cardTitle") : agent.name}
              hint={t("theme.cardHint")}
            >
              {/* 名称 + 版本 + 安装状态 */}
              <div className="flex items-center justify-between gap-2 mb-2">
                <div className="flex items-baseline gap-1.5 min-w-0">
                  <span className="text-[12px] font-semibold text-slate-900 truncate">
                    {agent.name}
                  </span>
                  {state.appVersion && (
                    <span className="text-[9px] text-slate-500 shrink-0">
                      {t("theme.version", { v: state.appVersion })}
                    </span>
                  )}
                </div>
                {state.needsReinstall ? (
                  <StatusBadge color="amber">
                    {t("theme.statusNeedsReinstall")}
                  </StatusBadge>
                ) : state.installed ? (
                  <StatusBadge color="emerald">
                    {t("theme.statusInstalled")}
                  </StatusBadge>
                ) : (
                  <StatusBadge color="sky">
                    {t("theme.statusNotInstalled")}
                  </StatusBadge>
                )}
              </div>

              {/* 状态横幅：升级失效 / Node 缺失 / 备份缺失 */}
              {state.needsReinstall && (
                <AlertBanner type="warning">
                  {t("theme.needsReinstallBanner")}
                </AlertBanner>
              )}
              {!state.nodeAvailable && (
                <AlertBanner type="warning">
                  {t("theme.nodeMissingBanner")}
                </AlertBanner>
              )}
              {state.installed && state.backupMissing && (
                <AlertBanner type="warning">
                  {t("theme.backupMissingBanner")}
                </AlertBanner>
              )}

              {/* 操作按钮：未安装 → 安装；已安装 → 重装 / 重启 / 还原（红色）；
                  换壁纸入口移至下方拖拽投放区（原生文件对话框在本应用上不可见） */}
              <div className="flex flex-wrap items-center gap-1.5">
                {!state.installed ? (
                  <BtnPrimary
                    onClick={() => setConfirm("install")}
                    disabled={installDisabled}
                  >
                    {t("theme.install")}
                  </BtnPrimary>
                ) : (
                  <>
                    <BtnPrimary
                      onClick={() => setConfirm("install")}
                      disabled={installDisabled}
                    >
                      {t("theme.reinstall")}
                    </BtnPrimary>
                    <BtnSecondary
                      onClick={() => setConfirmRestart(true)}
                      disabled={actionsDisabled}
                    >
                      {restarting ? t("theme.restarting") : t("theme.restartZcode")}
                    </BtnSecondary>
                    <BtnSecondary
                      onClick={() => setConfirm("uninstall")}
                      disabled={actionsDisabled}
                      className="bg-red-500/10! text-red-600! hover:bg-red-500/18!"
                    >
                      {t("theme.uninstall")}
                    </BtnSecondary>
                  </>
                )}
              </div>

              {/* 安装/还原进度：分阶段文案 + 百分比 + Rust 侧补充说明 */}
              {busy && (
                <div className="mt-2.5">
                  <div className="flex items-center justify-between text-[10px] text-slate-600/80 mb-1">
                    <span>
                      {(busy === "install"
                        ? t("theme.installing")
                        : t("theme.uninstalling")) +
                        (progress
                          ? ` · ${
                              progress.stage in STAGE_KEYS
                                ? t(STAGE_KEYS[progress.stage])
                                : progress.stage
                            }`
                          : "")}
                    </span>
                    <span className="num">
                      {progress ? `${Math.max(0, Math.min(100, Math.round(progress.percent)))}%` : ""}
                    </span>
                  </div>
                  <div className="h-1.5 rounded-full bg-slate-900/8 overflow-hidden">
                    <div
                      className={`h-full rounded-full transition-all duration-300 ${
                        progress?.stage === "error"
                          ? "bg-red-500"
                          : "bg-sky-500"
                      }`}
                      style={{
                        width: `${Math.max(0, Math.min(100, progress?.percent ?? 0))}%`,
                      }}
                    />
                  </div>
                  {progress?.detail && (
                    <div className="text-[9px] text-slate-500 mt-1 break-all">
                      {progress.detail}
                    </div>
                  )}
                </div>
              )}

              {/* Rust 侧附加说明（如未找到应用的原因） */}
              {state.detail && !busy && (
                <p className="text-[9px] text-slate-500 mt-2 leading-relaxed break-all">
                  {state.detail}
                </p>
              )}
            </SettingsCard>
          ))}

          {/* 壁纸库：点击即切换（热重载约 1 秒生效）；目录行支持刷新与清除；
              仅已安装且无需重装时展示 */}
          {state.installed && !state.needsReinstall && (
            <SettingsCard
              title={t("theme.libraryTitle")}
              hint={t("theme.libraryHint")}
              action={
                <BtnSecondary
                  onClick={refreshWallpapers}
                  disabled={actionsDisabled || wallpapersLoading}
                  className="px-2! py-0.5! text-[10px]!"
                >
                  {t("common.refresh")}
                </BtnSecondary>
              }
            >
              {/* 壁纸目录行：未设置时提示拖入文件夹；已设置时展示截断路径 + 清除 */}
              <div className="flex items-start justify-between gap-2 mb-2.5">
                <div className="min-w-0 flex-1">
                  <div className="text-[10px] text-slate-600">
                    {t("theme.libraryDirLabel")}
                  </div>
                  {params?.wallpaperDir ? (
                    <div
                      className="text-[10px] font-medium text-slate-800 truncate"
                      title={params.wallpaperDir}
                    >
                      {params.wallpaperDir}
                    </div>
                  ) : (
                    <div className="text-[9px] text-slate-500 leading-relaxed">
                      {t("theme.libraryDirEmpty")}
                    </div>
                  )}
                </div>
                {params?.wallpaperDir && (
                  <BtnSecondary
                    onClick={handleClearWallpaperDir}
                    disabled={actionsDisabled || settingDir}
                    className="px-2! py-0.5! text-[10px]! shrink-0"
                  >
                    {t("theme.libraryClearDir")}
                  </BtnSecondary>
                )}
              </div>

              {/* 列表：加载 / 空态 / 预览卡网格（16:9 预览 + 文件名，
                  当前项高亮，视频 hover 即播放预览） */}
              {wallpapersLoading && !wallpapers ? (
                <div className="text-[10px] text-slate-500 py-1.5 text-center">
                  {t("theme.libraryLoading")}
                </div>
              ) : wallpapers && wallpapers.length > 0 ? (
                <div className="grid grid-cols-2 gap-1.5">
                  {wallpapers.map((w) => (
                    <WallpaperCard
                      key={w.path}
                      entry={w}
                      label={
                        w.path === "default"
                          ? t("theme.defaultWallpaperName")
                          : w.fileName
                      }
                      current={isCurrentEntry(w, params?.wallpaperFile)}
                      selecting={selectingPath === w.path}
                      disabled={actionsDisabled || selectingPath !== null}
                      onSelect={() => handleSelectWallpaper(w)}
                    />
                  ))}
                </div>
              ) : (
                <div className="text-[9px] text-slate-500 leading-relaxed py-1.5 text-center border border-dashed border-slate-900/10 rounded-md">
                  {t("theme.libraryEmpty")}
                </div>
              )}
            </SettingsCard>
          )}

          {/* 拖拽投放区：拖入壁纸文件导入换壁纸 / 拖入文件夹设为壁纸目录
              （仅已安装且无需重装时展示；busy / 处理中降低透明度，
              实际忽略逻辑在 dragGuardRef 守卫里） */}
          {state.installed && !state.needsReinstall && (
            <div
              className={`rounded-lg border border-dashed p-4 text-center transition-colors ${
                dragOver
                  ? "border-sky-500 bg-sky-500/10"
                  : "border-slate-900/15 hover:border-slate-900/25"
              } ${actionsDisabled || changingWallpaper || settingDir ? "opacity-40" : ""}`}
            >
              <div className="text-[10px] text-slate-600">
                {changingWallpaper || settingDir
                  ? t("theme.dropWallpaperBusy")
                  : t("theme.dropWallpaperHint")}
              </div>
            </div>
          )}

          {/* 效果参数区：仅已安装且参数读取成功时可用 */}
          {state.installed && params && (
            <SettingsCard
              title={t("theme.paramsTitle")}
              hint={t("theme.paramsHint")}
              action={
                paramsSavedFlash ? (
                  <span className="text-[9px] text-emerald-600">
                    {t("theme.paramsSavedFlash")}
                  </span>
                ) : undefined
              }
            >
              <div className="flex flex-col gap-2.5">
                {SLIDERS.map((s) => (
                  <ParamSlider
                    key={s.key}
                    label={t(s.labelKey)}
                    hint={s.hintKey ? t(s.hintKey) : undefined}
                    value={toScale(params[s.key], s.scale)}
                    min={s.min}
                    max={s.max}
                    step={s.step}
                    format={s.format}
                    disabled={actionsDisabled}
                    onChange={(v) => handleSlider(s.key, v, s.scale)}
                  />
                ))}

                {/* 当前壁纸（换壁纸走上方壁纸库与拖拽投放区）；wallpaperFile
                    可能是绝对路径，展示取末段文件名、悬浮看全路径 */}
                <div className="pt-2 border-t border-slate-900/6">
                  <div className="text-[10px] text-slate-600">
                    {t("theme.currentWallpaper")}
                  </div>
                  <div
                    className="text-[10px] font-medium text-slate-800 truncate"
                    title={params.wallpaperFile ?? undefined}
                  >
                    {!params.wallpaperFile
                      ? t("theme.noWallpaper")
                      : params.wallpaperFile === "default.mp4"
                        ? t("theme.defaultWallpaperName")
                        : baseName(params.wallpaperFile)}
                  </div>
                </div>

                {/* 一键预设与恢复默认等宽两列：亮色壁纸适配只覆盖四项推荐
                    值，恢复默认重置全部滑块（均保留壁纸指向与壁纸目录）；
                    重启 ZCode 入口已移至顶部 Agent 应用卡片按钮区 */}
                <div className="grid grid-cols-2 gap-1.5">
                  <BtnSecondary
                    onClick={handleLightWallpaperPreset}
                    disabled={actionsDisabled}
                  >
                    {t("theme.lightWallpaperPreset")}
                  </BtnSecondary>
                  <BtnSecondary
                    onClick={handleResetParams}
                    disabled={actionsDisabled}
                  >
                    {t("theme.resetParams")}
                  </BtnSecondary>
                </div>
                <p className="text-[9px] text-slate-500 leading-relaxed">
                  {t("theme.lightWallpaperPresetDesc")}
                </p>
              </div>
            </SettingsCard>
          )}

          {/* 用量统计条区：独立于壁纸效果参数的配置区域（调整 ZCode 对话内
              每轮末尾统计条的字号与不透明度，并可开关每轮统计条与会话
              累计条），仅已安装且参数读取成功时可用；保存走同一防抖管道
              （set_agent_theme_params 整体落盘），保存成功反馈复用
              paramsSavedFlash */}
          {state.installed && params && (
            <SettingsCard
              title={t("theme.usageTitle")}
              hint={t("theme.usageHint")}
              action={
                paramsSavedFlash ? (
                  <span className="text-[9px] text-emerald-600">
                    {t("theme.paramsSavedFlash")}
                  </span>
                ) : undefined
              }
            >
              <div className="flex flex-col gap-2.5">
                {USAGE_SLIDERS.map((s) => (
                  <ParamSlider
                    key={s.key}
                    label={t(s.labelKey)}
                    hint={s.hintKey ? t(s.hintKey) : undefined}
                    value={toScale(params[s.key], s.scale)}
                    min={s.min}
                    max={s.max}
                    step={s.step}
                    format={s.format}
                    disabled={actionsDisabled}
                    onChange={(v) => handleSlider(s.key, v, s.scale)}
                  />
                ))}

                {/* 开关组（首行分隔线在滑块与开关组之间，两开关行之间
                    不再加分隔线）：每轮统计条开关（usage.js V19）——ZCode
                    对话内每轮回复末尾的统计条；字号/不透明度复用上方两个
                    滑块，开关经 variables.css 热重载生效（样式同设置页
                    既有 checkbox 模式） */}
                <label className="flex items-center justify-between gap-2 cursor-pointer pt-2 border-t border-slate-900/6">
                  <span className="min-w-0">
                    <span className="block text-[10px] text-slate-600">
                      {t("theme.usageTurnBar")}
                    </span>
                    <span className="block text-[9px] text-slate-500 leading-relaxed">
                      {t("theme.usageTurnBarHint")}
                    </span>
                  </span>
                  <input
                    type="checkbox"
                    checked={params.usageTurnBar}
                    disabled={actionsDisabled}
                    onChange={(e) => handleUsageTurnBar(e.target.checked)}
                    className="accent-sky-500 h-3 w-3 shrink-0 disabled:opacity-40"
                  />
                </label>

                {/* 会话累计条开关（usage.js V5）：固定悬浮于 ZCode 对话
                    输入框上方的会话级实时统计条，流式生成时动态跳动；
                    字号/不透明度复用上方两个滑块，开关经 variables.css
                    热重载生效 */}
                <label className="flex items-center justify-between gap-2 cursor-pointer pt-2">
                  <span className="min-w-0">
                    <span className="block text-[10px] text-slate-600">
                      {t("theme.usageSessionBar")}
                    </span>
                    <span className="block text-[9px] text-slate-500 leading-relaxed">
                      {t("theme.usageSessionBarHint")}
                    </span>
                  </span>
                  <input
                    type="checkbox"
                    checked={params.usageSessionBar}
                    disabled={actionsDisabled}
                    onChange={(e) => handleUsageSessionBar(e.target.checked)}
                    className="accent-sky-500 h-3 w-3 shrink-0 disabled:opacity-40"
                  />
                </label>

                {/* 符号图例：统计条各图标含义说明（与 usage.js 行格式
                    一一对应，方便对照实机读数） */}
                <p className="text-[9px] text-slate-500 leading-relaxed break-words">
                  {t("theme.usageLegend")}
                </p>
              </div>
            </SettingsCard>
          )}

          {/* 桌面宠物区（宠物设置唯一入口，配置源 pet.json/PetConfig）：
              总开关 + 注入版/悬浮窗形态二选一 + 形象选择 + 尺寸档位。
              配置读取成功即渲染（悬浮窗形态不依赖皮肤安装，未安装皮肤
              也能设置，选注入版时装完皮肤即出现）；改完经 set_pet_config
              即时生效——注入版参数经 variables.css 热重载约 1 秒生效、
              悬浮窗即时建/关窗；安装/卸载进行中禁用操作 */}
          {petCfg && (
            <SettingsCard
              title={t("theme.petTitle")}
              hint={t("theme.petHint")}
              action={
                <span className="text-[9px] text-slate-500">
                  {t("settings.instant")}
                </span>
              }
            >
              <div className="flex flex-col gap-2.5">
                {/* 宠物总开关：关 = 全关（注入版移除 DOM、悬浮窗关窗停
                    轮询）；开 = 按下方形态选择生效 */}
                <label className="flex items-center justify-between gap-2 cursor-pointer">
                  <span className="min-w-0">
                    <span className="block text-[10px] text-slate-600">
                      {t("theme.petEnabled")}
                    </span>
                    <span className="block text-[9px] text-slate-500 leading-relaxed">
                      {t("theme.petEnabledHint")}
                    </span>
                  </span>
                  <input
                    type="checkbox"
                    checked={petCfg.enabled}
                    disabled={actionsDisabled}
                    onChange={(e) =>
                      void applyPet({ ...petCfg, enabled: e.target.checked })
                    }
                    className="accent-sky-500 h-3 w-3 shrink-0 disabled:opacity-40"
                  />
                </label>

                {/* 形态选择 + 形象 + 尺寸：总开关关闭时降透明度并阻断
                    交互（保留设置值，重新开启即恢复） */}
                <div
                  className={`flex flex-col gap-2.5 pt-2 border-t border-slate-900/6 ${
                    petCfg.enabled ? "" : "opacity-40 pointer-events-none"
                  }`}
                >
                  {/* 形态二选一（默认注入版）：注入版渲染在 ZCode 对话页
                      （需已安装皮肤、随 variables.css 热重载，可拖拽移位）；
                      悬浮窗为独立透明置顶窗（不依赖皮肤） */}
                  <div className="flex items-center gap-2">
                    <span className="text-[10px] text-slate-700/55 shrink-0">
                      {t("theme.petModeLabel")}
                    </span>
                    <PillGroup className="flex-1">
                      {(
                        [
                          ["injected", "theme.petModeInjected"],
                          ["floating", "theme.petModeFloating"],
                        ] as [PetMode, MessageKey][]
                      ).map(([mode, key]) => (
                        <PillButton
                          key={mode}
                          active={petCfg.mode === mode}
                          disabled={actionsDisabled}
                          onClick={() => void applyPet({ ...petCfg, mode })}
                        >
                          {t(key)}
                        </PillButton>
                      ))}
                    </PillGroup>
                  </div>
                  <p className="text-[9px] text-slate-500 leading-relaxed">
                    {t("theme.petModeHint")}
                  </p>

                  <PetStyleSection
                    value={petCfg.style}
                    disabled={actionsDisabled}
                    onSelect={(id) => void applyPet({ ...petCfg, style: id })}
                    controller={customPets}
                  />

                  {/* 尺寸档位（屏高比例 1~5）：离散点击即时保存（set_pet_
                      config 本身即时生效，无需防抖），px 换算在 Rust 侧
                      按屏幕逻辑高完成 */}
                  <PetSizeLevelPicker
                    labelKey="theme.paramPetSize"
                    value={petCfg.size}
                    disabled={actionsDisabled}
                    onSelect={(level) => void applyPet({ ...petCfg, size: level })}
                  />
                  <p className="text-[9px] text-slate-500 leading-relaxed">
                    {t("theme.paramPetSizeHint")}
                  </p>
                </div>

                {/* 状态图例：宠物工作状态的含义说明 */}
                <p className="text-[9px] text-slate-500 leading-relaxed break-words">
                  {t("theme.petLegend")}
                </p>
              </div>
            </SettingsCard>
          )}
        </PageBody>
      </PageShell>

      {/* 安装/还原二次确认浮层（复刻 AccountsCard ConfirmDialog 样式） */}
      {confirm && !busy && (
        <ConfirmDialog
          title={
            confirm === "install"
              ? t("theme.confirmInstallTitle")
              : t("theme.confirmUninstallTitle")
          }
          desc={confirmDesc(confirm)}
          confirmText={
            confirm === "install" ? t("theme.install") : t("theme.uninstall")
          }
          danger={confirm === "uninstall"}
          showMacUpdateNote={isMac}
          onCancel={() => setConfirm(null)}
          onConfirm={() => runAction(confirm)}
        />
      )}

      {/* 重启 ZCode 二次确认浮层（同款样式，非危险操作走蓝色确认键；
          重启会退出 ZCode，需提示先保存进行中的对话） */}
      {confirmRestart && !busy && !restarting && (
        <ConfirmDialog
          title={t("theme.confirmRestartTitle")}
          desc={t("theme.confirmRestartDesc")}
          confirmText={t("theme.restartZcode")}
          danger={false}
          onCancel={() => setConfirmRestart(false)}
          onConfirm={handleRestartZcode}
        />
      )}
    </div>
  );
}

/**
 * 壁纸库预览卡：上方 16:9 预览区 + 底部文件名单行截断。
 * - 图片用 `<img>` 直接展示；视频用 `<video preload="metadata">` 首帧即
 *   缩略，hover 时从头播放预览、离开暂停并把进度归零（下次 hover 仍从
 *   首帧开始，避免每次进入都停在半途画面）；
 * - 预览源经 convertFileSrc 转 asset://（Rust 侧 list 时已动态放行目录）；
 *   视频源额外追加 `#t=0.1` 媒体片段指示，让 webview 在 preload="metadata"
 *   下即渲染首帧、避免初始白屏（不影响 hover 播放与 asset 请求）；
 * - 加载失败（文件缺失 / 默认壁纸未产出 / 格式不支持）退化为类型徽章占位；
 * - 当前项高亮与点击切换逻辑由父组件保持（current / onSelect 透传）。
 */
function WallpaperCard({
  entry,
  label,
  current,
  selecting,
  disabled,
  onSelect,
}: {
  entry: WallpaperEntry;
  label: string;
  current: boolean;
  selecting: boolean;
  disabled: boolean;
  onSelect: () => void;
}) {
  const { t } = useI18n();
  const videoRef = useRef<HTMLVideoElement | null>(null);
  const [failed, setFailed] = useState(false);
  // React 的 JSX muted 属性在部分渲染路径只设 attribute 不设 property，
  // 而自动播放要求 muted property 为 true，ref 回调里显式兜底
  const bindVideo = (el: HTMLVideoElement | null) => {
    videoRef.current = el;
    if (el) el.muted = true;
  };
  const handleEnter = () => {
    const v = videoRef.current;
    if (v) {
      v.currentTime = 0;
      // 静默失败：自动播放被拒时保留首帧静态预览即可
      v.play().catch(() => {});
    }
  };
  const handleLeave = () => {
    const v = videoRef.current;
    if (v) {
      v.pause();
      v.currentTime = 0;
    }
  };
  const src = convertFileSrc(entry.previewPath);
  // 视频源追加媒体片段指示（#t=0.1）：提示 webview 在 preload="metadata"
  // 阶段即定位并渲染该时间点帧，修复网格初始白屏；不影响 hover 播放
  // 与 asset:// 请求。图片分支保持原样
  const videoSrc = entry.kind === "video" ? `${src}#t=0.1` : src;
  return (
    <button
      onClick={onSelect}
      disabled={disabled}
      onMouseEnter={entry.kind === "video" ? handleEnter : undefined}
      onMouseLeave={entry.kind === "video" ? handleLeave : undefined}
      title={entry.path === "default" ? undefined : entry.path}
      className={`flex flex-col overflow-hidden rounded-md border text-left transition-colors disabled:opacity-40 ${
        current
          ? "border-sky-500 bg-sky-500/10"
          : "border-slate-900/10 bg-slate-900/4 hover:border-slate-900/25"
      }`}
    >
      <div className="relative aspect-video w-full bg-slate-900/5">
        {failed ? (
          <div className="absolute inset-0 flex flex-col items-center justify-center gap-0.5 text-slate-400">
            <span
              className="text-[14px] leading-none"
              title={t(
                entry.kind === "image"
                  ? "theme.libraryKindImage"
                  : "theme.libraryKindVideo"
              )}
            >
              {entry.kind === "image" ? "🖼" : "🎬"}
            </span>
            <span className="text-[8px]">{t("theme.previewUnavailable")}</span>
          </div>
        ) : entry.kind === "image" ? (
          <img
            src={src}
            alt={label}
            loading="lazy"
            onError={() => setFailed(true)}
            className="absolute inset-0 h-full w-full object-cover"
          />
        ) : (
          <video
            ref={bindVideo}
            src={videoSrc}
            muted
            loop
            playsInline
            preload="metadata"
            onError={() => setFailed(true)}
            className="absolute inset-0 h-full w-full object-cover"
          />
        )}
        {selecting && (
          <span className="absolute right-1 top-1 rounded bg-black/45 px-1 py-0.5 text-[8px] text-white">
            {t("theme.selecting")}
          </span>
        )}
      </div>
      <div className="flex items-center gap-1 px-1.5 py-1">
        <span className="min-w-0 flex-1 truncate text-[10px] font-medium text-slate-800">
          {label}
        </span>
        {current && (
          <span className="shrink-0 text-[9px] font-semibold text-sky-600">✓</span>
        )}
      </div>
    </button>
  );
}

/** 单个效果参数滑块：标签 + 当前值 + range（样式对齐设置页透明度滑块）；
 *  hint 为可选的滑块下方小字说明（仅部分可读性参数提供） */
function ParamSlider({
  label,
  hint,
  value,
  min,
  max,
  step,
  format,
  disabled,
  onChange,
}: {
  label: string;
  hint?: string;
  value: number;
  min: number;
  max: number;
  step: number;
  format: (v: number) => string;
  disabled?: boolean;
  onChange: (v: number) => void;
}) {
  return (
    <label className="block">
      <div className="flex items-center justify-between mb-1">
        <span className="text-[10px] text-slate-600">{label}</span>
        <span className="num text-[10px] font-medium text-slate-800">
          {format(value)}
        </span>
      </div>
      <input
        type="range"
        min={min}
        max={max}
        step={step}
        value={value}
        disabled={disabled}
        onChange={(e) => onChange(parseFloat(e.target.value))}
        className="accent-sky-500 w-full disabled:opacity-40"
      />
      {hint && (
        <div className="text-[9px] text-slate-500 leading-relaxed mt-0.5">
          {hint}
        </div>
      )}
    </label>
  );
}

/**
 * 宠物形象像素预览：以 CSS grid 逐像素渲染形象清单里的精简帧数据
 * （16×16，"." 透明、"1".."8" 调色板下标），与 ZCode 内 pet.js 的
 * canvas 渲染观感一致（正方形网格最近邻观感）。静态一次性渲染，
 * 无动画无交互；256 个格子均为无样式或仅背景色的空 div，开销可忽略。
 */
/** 二次确认浮层（本地复刻 AccountsCard/SyncPanel 样式，danger 时红色确认键） */
function ConfirmDialog({
  title,
  desc,
  confirmText,
  danger,
  showMacUpdateNote,
  onCancel,
  onConfirm,
}: {
  title: string;
  desc: string;
  confirmText: string;
  danger: boolean;
  /** macOS 专属：正文下方追加"内置更新将不可用"提示与官网链接（Windows 不展示） */
  showMacUpdateNote?: boolean;
  onCancel: () => void;
  onConfirm: () => void;
}) {
  const { t } = useI18n();
  /** 用系统浏览器打开官网（openUrl 失败退化为 window.open，参照 UpdaterCard） */
  const handleOpenSite = () => {
    openUrl(ZCODE_OFFICIAL_SITE).catch(() =>
      window.open(ZCODE_OFFICIAL_SITE, "_blank")
    );
  };
  return (
    <div className="absolute inset-0 z-50 flex items-center justify-center bg-black/30 rounded-2xl">
      <div className="mx-4 w-full rounded-lg bg-elevated border border-slate-900/10 p-3 shadow-xl">
        <div className="text-[12px] font-semibold text-slate-900 mb-1">
          {title}
        </div>
        <p className="text-[10px] text-slate-700/65 leading-relaxed mb-2 whitespace-pre-line">
          {desc}
        </p>
        {/* macOS 更新影响提示：警示色文案 + 官网链接（系统浏览器打开） */}
        {showMacUpdateNote && (
          <p className="text-[10px] text-amber-600 leading-relaxed mb-2">
            {t("theme.confirmMacUpdateNote")}
            <br />
            <button
              onClick={handleOpenSite}
              className="text-sky-600 underline underline-offset-2 hover:text-sky-700 transition-colors"
            >
              {t("theme.zcodeOfficialSite")}
            </button>
          </p>
        )}
        <div className="flex gap-1.5">
          <button
            onClick={onCancel}
            className="flex-1 text-[11px] py-1 rounded-md bg-slate-900/5 text-slate-700/70 hover:bg-slate-900/10 transition-colors"
          >
            {t("common.cancel")}
          </button>
          <button
            onClick={onConfirm}
            className={`flex-1 text-[11px] py-1 rounded-md text-white transition-colors ${
              danger
                ? "bg-red-500 hover:bg-red-600"
                : "bg-sky-500 hover:bg-sky-600"
            }`}
          >
            {confirmText}
          </button>
        </div>
      </div>
    </div>
  );
}

/**
 * 皮肤 · 免注入子页：会话悬浮窗（HUD）与桌面宠物两张设置卡的独立页面。
 *
 * 硬性约束：本页不得调用任何注入状态检测类 API（get_agent_theme_state
 * / list_agent_wallpapers / get_agent_theme_params 等 Tauri 命令）——
 * 打开本页不触发 ZCode 安装目录扫描（原皮肤页挂载即扫描导致首屏慢，
 * 是拆页的核心动机）。悬浮窗
 * 与宠物（悬浮窗形态）本就不依赖皮肤安装；宠物选「内置版（注入）」
 * 形态时需已安装皮肤才出现，但设置本身不需要检测安装状态。
 *
 * 拖放导入：本页只承载宠物形象导入（Petdex 包 zip/pet.json 与裸图集
 * png/webp，见 petStyles 的 PET_IMPORT_*_RE）；壁纸导入语义留在需注入
 * 子页（ThemePanel）。
 */
import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import {
  getPetConfig,
  getSessionHudConfig,
  setPanelSticky,
  setPetConfig,
  setSessionHudConfig,
} from "./api";
import {
  AlertBanner,
  PageBody,
  PageHeader,
  PageShell,
  PillButton,
  PillGroup,
  SettingsCard,
} from "./layout";
import { useI18n, type MessageKey } from "./i18n";
import {
  PET_IMPORT_FILE_RE,
  PET_IMPORT_IMAGE_RE,
  PetSizeLevelPicker,
  PetStyleSection,
  useCustomPets,
} from "./petStyles";
import type { PetConfig, PetMode, SessionHudConfig } from "./types";

/** 兼容 Windows 分隔符取路径的最后一段（与 ThemePanel 同款工具） */
const baseName = (p: string) => p.split(/[\\/]/).pop() ?? p;

/**
 * 桌面宠物大小：屏高比例档位 1~5（5.5%~15%），不用滑杆——渲染时 Rust
 * 侧按主显示器逻辑高换算成整数 px 写入 --zbar-pet-size（pet.js 画布
 * CSS 尺寸直接消费，换机器观感一致），UI 用 petStyles 的
 * PetSizeLevelPicker 分段控件（与字号档位同风格）；档位存 PetConfig
 * （pet.json），改完经 setPetConfig 即时生效。
 */

interface Props {
  onBack: () => void;
}

export function SkinStandalonePanel({ onBack }: Props) {
  const { t } = useI18n();
  const [error, setError] = useState<string | null>(null);
  const [flash, setFlash] = useState<string | null>(null);
  // 成功反馈自动消失 timer
  const flashTimer = useRef<number | undefined>(undefined);
  // onDragDropEvent 的 effect 闭包只注册一次，经 ref 转发到最新处理函数
  const dropHandlerRef = useRef<(paths: string[]) => void>(() => {});
  // 宠物导入处理锁：导入进行中忽略新 drop（useCustomPets.importFromPath
  // 内部亦有防重入，这里同步置位提前拦截；原皮肤页 dragGuardRef.locked
  // 的本页等价物，免注入页无壁纸导入语义，不需要 active 守卫）
  const dropLockRef = useRef(false);

  // ===== 桌面宠物（pet.json/PetConfig 唯一真相源：总开关 + 内置版/
  // 悬浮窗形态二选一 + 形象/尺寸，改完 setPetConfig 即时生效）=====
  const [petCfg, setPetCfg] = useState<PetConfig | null>(null);
  // 最新宠物配置镜像（拖放兜底等一次性闭包读不到最新 state）
  const petCfgRef = useRef<PetConfig | null>(null);

  // 自定义宠物：清单/拖放导入/删除共享控制器；导入或删除后重拉宠物
  // 配置（删除正在使用的宠物时 Rust 侧会把选中回退内建默认形象并热
  // 生效，面板需要重新读取才能同步高亮）
  const refreshPetCfgQuiet = () => {
    getPetConfig()
      .then((c) => {
        petCfgRef.current = c;
        setPetCfg(c);
      })
      .catch(() => {});
  };
  const customPets = useCustomPets(refreshPetCfgQuiet);

  // ===== 会话悬浮窗（session-hud.json/SessionHudConfig 唯一真相源：
  // 总开关 + 活跃窗口档位 + 显示项，改完 setSessionHudConfig 即时生效；
  // 不依赖皮肤安装）。透明度/字体缩放不在本页——两者入口已移入悬浮窗
  // 自身的设置面板（见 session-hud.html / session-hud-main.ts），配置
  // 字段 opacity/fontScale 仍由悬浮窗侧读写。滑块防抖管道（宽度 V3 /
  // 透明度 V4 相继移除）随之清空，不再保留最新配置镜像
  const [hudCfg, setHudCfg] = useState<SessionHudConfig | null>(null);
  // applyHud 提交代数：仅最新一次提交的响应可写回状态，防止在途慢响
  // 应（先发出的提交后返回）覆盖用户已提交/拖动到的新值
  const hudApplySeq = useRef(0);

  // 挂载：开启面板粘滞（拖宠物包进窗口导入时需切窗，失焦不自动隐藏）
  // + 订阅 Tauri 拖放事件；卸载恢复默认失焦隐藏并注销订阅（参照
  // ThemePanel 的 disposed 模式）
  useEffect(() => {
    setPanelSticky(true).catch(() => {});
    let unlisten: (() => void) | undefined;
    let disposed = false;
    // drop 取首个文件走宠物导入流程（enter/over 无投放区高亮，与原
    // 皮肤页未安装态一致——宠物导入本来就没有专属投放区）
    getCurrentWebview()
      .onDragDropEvent((event) => {
        if (event.payload.type === "drop") {
          dropHandlerRef.current(event.payload.paths);
        }
      })
      .then((fn) => {
        // 订阅完成前组件已卸载：立即注销，避免泄漏
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {
        /* 订阅失败仅退化为无拖放入口，不影响卡片内其余操作 */
      });
    return () => {
      disposed = true;
      unlisten?.();
      // 关闭本页必须恢复默认失焦隐藏，避免粘滞标志泄漏
      setPanelSticky(false).catch(() => {});
    };
  }, []);

  // 宠物配置加载（挂载即拉取；失败不阻塞页面其余功能，宠物卡停在本地
  // 面板不渲染——悬浮窗形态不依赖皮肤安装，未安装皮肤也可设置，选内
  // 置版（注入）形态时装完皮肤即出现）
  useEffect(() => {
    getPetConfig()
      .then((c) => {
        petCfgRef.current = c;
        setPetCfg(c);
      })
      .catch((e) => setError(t("theme.petLoadFail", { msg: String(e) })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 会话悬浮窗配置加载（挂载即拉取；失败不阻塞页面其余功能，卡片停在
  // 本地面板不渲染——该功能不依赖皮肤安装）
  useEffect(() => {
    getSessionHudConfig()
      .then((c) => setHudCfg(c))
      .catch((e) => setError(t("theme.hudLoadFail", { msg: String(e) })));
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  // 卸载清理：冲掉未触发的成功反馈清除 timer
  useEffect(() => {
    return () => {
      if (flashTimer.current !== undefined) {
        window.clearTimeout(flashTimer.current);
      }
    };
  }, []);

  /** 轻量成功反馈：顶部绿色横幅，4s 后自动消失（与 ThemePanel 同款） */
  const showFlash = (text: string) => {
    setFlash(text);
    if (flashTimer.current !== undefined) {
      window.clearTimeout(flashTimer.current);
    }
    flashTimer.current = window.setTimeout(() => setFlash(null), 4000);
  };

  /**
   * 应用宠物配置（pet.json 唯一真相源，set_pet_config 改完即生效，不走
   * 参数防抖管道）：乐观更新 + 失败回读回滚——
   * - 总开关/形态切换：内置版经 variables.css 热重载约 1 秒生效（需已
   *   安装皮肤）；悬浮窗即时建/关窗；
   * - 形象/尺寸变化：内置版热切换重建画布，悬浮窗同步尺寸热推参数。
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
   * 应用会话悬浮窗配置（session-hud.json 唯一真相源，set_session_hud_
   * config 改完即生效，不走参数防抖管道）：乐观更新 + 失败回读回滚——
   * 总开关切换即时建/关窗并启停轮询；显示项/活跃档位经
   * zbar://session-hud-params 热推悬浮窗即时生效（档位影响下一轮查询）。
   * 透明度与字体缩放不在本页（入口在悬浮窗自身的设置面板，各有专用轻量
   * 命令，只改对应字段）。
   */
  const applyHud = async (next: SessionHudConfig) => {
    setHudCfg(next);
    // 记下提交代数：返回时仅当代数仍为最新才写回（快速连续提交场景下，
    // 先发出的慢响应不得覆盖后发出的新值）
    const seq = ++hudApplySeq.current;
    try {
      const back = await setSessionHudConfig(next);
      if (seq !== hudApplySeq.current) return;
      setHudCfg(back);
    } catch (e) {
      if (seq !== hudApplySeq.current) return;
      setError(t("theme.hudApplyFail", { msg: String(e) }));
      try {
        const back = await getSessionHudConfig();
        if (seq !== hudApplySeq.current) return;
        setHudCfg(back);
      } catch {
        /* 回读失败保持当前态（下次切换再对齐） */
      }
    }
  };

  // 注：悬浮窗宽度滑块已移除（V3）——窗口改为可自由拖拽调整大小，
  // 尺寸由 Rust 侧 Resized 挂点持久化到 session-hud.json 的 width/
  // height（设置卡不再提供宽度入口）；透明度滑块与字体滑块也已移除
  //（V4）——两者入口在悬浮窗自身的设置面板内，本页只保留总开关/
  // 活跃档位/显示项

  /**
   * 处理拖放（免注入页版）：多文件时只取第一个，本页只承载宠物形象
   * 导入——Petdex 包（zip / pet.json，PET_IMPORT_FILE_RE）与裸图集
   * （png/webp，PET_IMPORT_IMAGE_RE）都路由给宠物导入；本页无壁纸
   * 导入语义，不存在抢占问题（原皮肤页"已安装时 png/webp 让给壁纸"
   * 的分流只在需注入子页保留）。其余文件（壁纸视频/文件夹等）静默
   * 忽略，交给对应子页处理。
   */
  const importPetFromDrop = async (path: string) => {
    if (dropLockRef.current) return;
    dropLockRef.current = true;
    try {
      const err = await customPets.importFromPath(path);
      // P2-3：宠物卡未渲染（配置读取失败）时导入结果无展示位，经
      // 全局反馈通道兜底（成功走 flash、失败走 error 条）
      if (petCfgRef.current === null) {
        if (err === null) showFlash(t("theme.petImportDone"));
        else setError(err);
      }
    } finally {
      dropLockRef.current = false;
    }
  };
  const handleDrop = async (paths: string[]) => {
    if (paths.length === 0) return;
    const fileName = baseName(paths[0]);
    if (
      PET_IMPORT_FILE_RE.test(fileName) ||
      PET_IMPORT_IMAGE_RE.test(fileName)
    ) {
      await importPetFromDrop(paths[0]);
    }
  };
  dropHandlerRef.current = handleDrop;

  return (
    <PageShell>
      <PageHeader title={t("theme.sectionStandalone")} onBack={onBack} />
      <PageBody className="page-stack">
        {error && <AlertBanner>{error}</AlertBanner>}
        {flash && <AlertBanner type="success">{flash}</AlertBanner>}

        {/* 会话悬浮窗区（Session HUD 设置入口，配置源
            session-hud.json/SessionHudConfig）：总开关 + 活跃窗口档位 +
            显示项勾选（字体缩放与透明度入口在悬浮窗自身的设置面板内，
            见 session-hud.html）。配置读取成功即渲染（不依赖皮肤安装）；
            改完经 set_session_hud_config 即时生效——开关即时建/关窗并
            启停轮询，其余经参数事件热推悬浮窗 */}
        {hudCfg && (
          <SettingsCard
            title={t("theme.hudTitle")}
            hint={t("theme.hudHint")}
            action={
              <span className="text-[9px] text-slate-500">
                {t("settings.instant")}
              </span>
            }
          >
            <div className="flex flex-col gap-2.5">
              {/* 总开关：关 = 关窗停轮询；开 = 建独立透明置顶悬浮窗 */}
              <label className="flex items-center justify-between gap-2 cursor-pointer">
                <span className="min-w-0">
                  <span className="block text-[10px] text-slate-600">
                    {t("theme.hudEnabled")}
                  </span>
                  <span className="block text-[9px] text-slate-500 leading-relaxed">
                    {t("theme.hudEnabledHint")}
                  </span>
                </span>
                <input
                  type="checkbox"
                  checked={hudCfg.enabled}
                  onChange={(e) =>
                    void applyHud({ ...hudCfg, enabled: e.target.checked })
                  }
                  className="accent-sky-500 h-3 w-3 shrink-0"
                />
              </label>

              {/* 活跃窗口档位 + 显示项：总开关关闭时降透明度并阻断交互
                   （保留设置值，重新开启即恢复） */}
              <div
                className={`flex flex-col gap-2.5 pt-2 border-t border-slate-900/6 ${
                  hudCfg.enabled ? "" : "opacity-40 pointer-events-none"
                }`}
              >
                {/* 活跃窗口档位（离散点击即时保存，与宠物尺寸档位同
                    交互）：判定会话"最近有活动"的时间窗 */}
                <div className="flex items-center gap-2">
                  <span className="text-[10px] text-slate-700/55 shrink-0">
                    {t("theme.hudWindowLabel")}
                  </span>
                  <PillGroup className="flex-1">
                    {(
                      [
                        [5, "theme.hudWindow5m"],
                        [10, "theme.hudWindow10m"],
                        [30, "theme.hudWindow30m"],
                        [0, "theme.hudWindowAll"],
                      ] as [number, MessageKey][]
                    ).map(([minutes, key]) => (
                      <PillButton
                        key={key}
                        active={hudCfg.windowMinutes === minutes}
                        onClick={() =>
                          void applyHud({ ...hudCfg, windowMinutes: minutes })
                        }
                      >
                        {t(key)}
                      </PillButton>
                    ))}
                  </PillGroup>
                </div>
                <p className="text-[9px] text-slate-500 leading-relaxed">
                  {t("theme.hudWindowHint")}
                </p>

                {/* 显示项勾选（首行分隔线，样式同设置页既有 checkbox
                    模式）：勾选即时热推悬浮窗重建列表。数据行整行开关
                    （Σ/↑/↓/⟲/×/速度/TTFT 字段口径对齐注入版会话条，
                    不再单设上下文条开关） */}
                {(
                  [
                    ["showTokens", "theme.hudShowTokens", "theme.hudShowTokensHint"],
                    ["showModel", "theme.hudShowModel", "theme.hudShowModelHint"],
                  ] as [
                    keyof SessionHudConfig,
                    MessageKey,
                    MessageKey
                  ][]
                ).map(([field, labelKey, hintKey]) => (
                  <label
                    key={field}
                    className="flex items-center justify-between gap-2 cursor-pointer pt-2 border-t border-slate-900/6"
                  >
                    <span className="min-w-0">
                      <span className="block text-[10px] text-slate-600">
                        {t(labelKey)}
                      </span>
                      <span className="block text-[9px] text-slate-500 leading-relaxed">
                        {t(hintKey)}
                      </span>
                    </span>
                    <input
                      type="checkbox"
                      checked={Boolean(hudCfg[field])}
                      onChange={(e) =>
                        void applyHud({
                          ...hudCfg,
                          [field]: e.target.checked,
                        })
                      }
                      className="accent-sky-500 h-3 w-3 shrink-0"
                    />
                  </label>
                ))}
              </div>
            </div>
          </SettingsCard>
        )}

        {/* 桌面宠物区（宠物设置唯一入口，配置源 pet.json/PetConfig）：
            总开关 + 内置版/悬浮窗形态二选一 + 形象选择 + 尺寸档位。
            配置读取成功即渲染（悬浮窗形态不依赖皮肤安装，未安装皮肤
            也能设置，选内置版时装完皮肤即出现）；改完经 set_pet_config
            即时生效——内置版参数经 variables.css 热重载约 1 秒生效、
            悬浮窗即时建/关窗 */}
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
              {/* 宠物总开关：关 = 全关（内置版移除 DOM、悬浮窗关窗停
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
                  onChange={(e) =>
                    void applyPet({ ...petCfg, enabled: e.target.checked })
                  }
                  className="accent-sky-500 h-3 w-3 shrink-0"
                />
              </label>

              {/* 形态选择 + 形象 + 尺寸：总开关关闭时降透明度并阻断
                  交互（保留设置值，重新开启即恢复）。本页无安装流程，
                  不需要原皮肤页"安装/卸载进行中禁用"的联动 */}
              <div
                className={`flex flex-col gap-2.5 pt-2 border-t border-slate-900/6 ${
                  petCfg.enabled ? "" : "opacity-40 pointer-events-none"
                }`}
              >
                {/* 形态二选一（默认内置版）：内置版渲染在 ZCode 对话页
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
                  onSelect={(id) => void applyPet({ ...petCfg, style: id })}
                  controller={customPets}
                />

                {/* 尺寸档位（屏高比例 1~5）：离散点击即时保存（set_pet_
                    config 本身即时生效，无需防抖），px 换算在 Rust 侧
                    按屏幕逻辑高完成 */}
                <PetSizeLevelPicker
                  labelKey="theme.paramPetSize"
                  value={petCfg.size}
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
  );
}

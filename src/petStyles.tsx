/**
 * 桌面宠物形象清单（共享模块）：皮肤页宠物卡（ThemePanel，宠物设置的
 * 唯一入口，注入版/悬浮窗两形态共用 pet.json 配置）使用。
 *
 * V8 起全部形象统一为 Petdex 图集形态（内建 cat/bot 字符网格形象已随
 * 核心渲染收敛移除）：「智谱 Z 娘」为软件内置形象（随安装包分发、默认
 * 选中、不可删除），与用户自定义宠物共用 custom:<id> 选中值和图集渲
 * 染通道，缩略图统一由 Rust 侧生成（idle 行首帧）。
 *
 * 自定义宠物（Petdex 格式导入）：useCustomPets 提供清单/拖放导入/删除
 * 的共享状态与操作，PetStyleSection 渲染「内建（智谱娘）+ 自定义」分组
 * 选择器（内建卡无删除按钮；自定义卡带删除按钮与格式角标，不加载全量
 * 图集）。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "./i18n";
import { deleteCustomPet, importPet, listCustomPets } from "./api";
import { PET_SIZE_LEVEL_PCT, type CustomPetEntry } from "./types";
import { PillGroup, PillButton } from "./layout";

// ===== 自定义宠物（Petdex 导入）=====

/** 宠物导入文件（拖放路由用）：zip 包 / pet.json 元信息（皮肤页的
 *  png/webp 投放已安装时保留壁纸导入语义） */
export const PET_IMPORT_FILE_RE = /\.(zip|json)$/i;
/** 宠物导入图集文件（拖放路由用）：皮肤页仅在未安装皮肤（壁纸导入
 *  不可用）时把 png/webp 路由给宠物导入（与原设置页语义一致），避免
 *  抢占已安装用户的壁纸导入主流程 */
export const PET_IMPORT_IMAGE_RE = /\.(png|webp)$/i;

/** 宠物尺寸档位名词条（与 PET_SIZE_LEVEL_PCT 下标一一对应，"默认"=档 3） */
const PET_SIZE_LEVEL_NAME_KEYS: ReadonlyArray<MessageKey> = [
  "settings.petSizeLevel1",
  "settings.petSizeLevel2",
  "settings.petSizeLevel3",
  "settings.petSizeLevel4",
  "settings.petSizeLevel5",
];

/**
 * 宠物尺寸档位选择器（屏高比例档位 1~5）：替代旧 48~128px 滑杆——按
 * 屏幕高度百分比定档（5.5%~15%），高分屏/低分屏观感一致，px 换算在
 * Rust 侧完成。分段控件与字号/窗口大小的 PillGroup 风格一致；离散点击
 * 即时保存（set_pet_config 本身即时生效，无滑杆拖动的防抖需求）。
 */
export function PetSizeLevelPicker({
  labelKey,
  value,
  disabled,
  onSelect,
}: {
  /** 标签词条（当前仅皮肤页 "theme.paramPetSize" 使用） */
  labelKey: MessageKey;
  /** 当前档位（1~5；越界值不高亮任何档） */
  value: number;
  disabled?: boolean;
  /** 选中档位（1~5） */
  onSelect: (level: number) => void;
}) {
  const { t } = useI18n();
  return (
    <div className="flex items-center gap-2">
      <span className="text-[10px] text-slate-700/55 shrink-0">{t(labelKey)}</span>
      <PillGroup className="flex-1">
        {PET_SIZE_LEVEL_PCT.map((_, i) => {
          const level = i + 1;
          return (
            <PillButton
              key={level}
              active={value === level}
              disabled={disabled}
              onClick={() => onSelect(level)}
            >
              {t(PET_SIZE_LEVEL_NAME_KEYS[i])}
            </PillButton>
          );
        })}
      </PillGroup>
    </div>
  );
}

/** 宠物形象的 style 值（pet_style / PetConfig.style 持久化形态）：内建
 *  智谱娘与用户自定义宠物共用 custom:<id> 通道（Rust 侧默认值同源） */
export const customStyleValue = (id: string) => `custom:${id}`;

/** useCustomPets 的控制器形态（PetStyleSection 与拖放路由共用） */
export interface CustomPetsController {
  /** 宠物清单（按 id 排序，含内置智谱娘，builtin 字段区分分组） */
  pets: CustomPetEntry[];
  /** 导入进行中（拖放处理期间禁用重入） */
  importing: boolean;
  /** 最近一次导入/删除失败的中文错误（null = 无） */
  error: string | null;
  /** 重新拉取清单（导入/删除成功后内部已自动刷新，一般无需手调） */
  refresh: () => Promise<void>;
  /** 拖放导入入口：null = 成功；字符串 = 中文错误信息（供宿主面板在
   *  宠物卡未渲染时经全局反馈通道兜底展示，P2-3） */
  importFromPath: (path: string) => Promise<string | null>;
  /** 删除自定义宠物（确认浮层在内部弹出；内置形象无删除入口） */
  removeById: (id: string, name: string) => Promise<void>;
}

/**
 * 宠物清单的共享状态与操作（皮肤页宠物卡持一份实例，导入/删除后
 * 刷新清单并经 onChanged 回调让宿主面板重拉宠物配置——删除正在使用
 * 的宠物时 Rust 侧会把选中回退默认形象（内置智谱娘），面板需要重新
 * 读取才能同步高亮）。
 */
export function useCustomPets(onChanged?: () => void): CustomPetsController {
  const { t } = useI18n();
  const [pets, setPets] = useState<CustomPetEntry[]>([]);
  const [importing, setImporting] = useState(false);
  const [error, setError] = useState<string | null>(null);
  // onChanged 回调身份不稳定（面板每次渲染新建），经 ref 转发避免
  // importFromPath/removeById 频繁重建
  const changedRef = useRef(onChanged);
  changedRef.current = onChanged;

  const refresh = useCallback(async () => {
    try {
      setPets(await listCustomPets());
    } catch {
      /* 清单读取失败静默：选择器仍显示内置形象占位，不阻塞面板 */
    }
  }, []);

  useEffect(() => {
    void refresh();
  }, [refresh]);

  const importFromPath = useCallback(
    async (path: string): Promise<string | null> => {
      if (importing) return t("theme.petImporting"); // 处理中防重入
      setImporting(true);
      setError(null);
      try {
        await importPet(path);
        await refresh();
        changedRef.current?.();
        return null;
      } catch (e) {
        const msg = t("theme.petImportFail", { msg: String(e) });
        setError(msg);
        return msg;
      } finally {
        setImporting(false);
      }
    },
    [importing, refresh, t]
  );

  const removeById = useCallback(
    async (id: string, name: string) => {
      if (!window.confirm(t("theme.petDeleteConfirm", { name }))) return;
      setError(null);
      try {
        await deleteCustomPet(id);
        await refresh();
        changedRef.current?.();
      } catch (e) {
        setError(t("theme.petDeleteFail", { msg: String(e) }));
      }
    },
    [t]
  );

  return { pets, importing, error, refresh, importFromPath, removeById };
}

/** 形象缩略图（Rust 生成的 idle 行首帧，64×70 内等比 PNG）与名称 */
function PetThumb({ entry }: { entry: CustomPetEntry }) {
  return (
    <>
      {entry.thumb ? (
        <img
          src={entry.thumb}
          alt=""
          style={{ imageRendering: "pixelated" }}
          className="max-w-[64px] max-h-[70px]"
          draggable={false}
        />
      ) : (
        /* 缩略图生成失败的占位（宠物仍可正常选用） */
        <span className="w-10 h-10 rounded bg-slate-900/8" />
      )}
      <span className="text-[9px] text-slate-600 px-1 truncate max-w-full">
        {entry.name}
      </span>
    </>
  );
}

/**
 * 宠物形象选择器（内建 + 自定义分组 + 导入区），皮肤页宠物卡使用：
 * - 内建组：内置「智谱 Z 娘」缩略图卡（Rust 启动时释放到宠物库，默认
 *   选中，无删除按钮）；
 * - 自定义组：Rust 生成的 idle 首帧缩略图卡（带删除按钮）；
 * - 导入区：拖放目标提示（原生文件对话框在 Accessory 应用不可用，
 *   与壁纸导入同样走 Tauri 拖放事件，由宿主面板路由到
 *   controller.importFromPath；png/webp 投放仅皮肤未安装时路由宠物
 *   导入，见 PET_IMPORT_IMAGE_RE）。
 */
export function PetStyleSection({
  value,
  disabled,
  onSelect,
  controller,
}: {
  /** 当前选中形象 id（custom:zhipu-z-niang / custom:<id>） */
  value: string;
  /** 操作禁用（皮肤页安装进行中等） */
  disabled?: boolean;
  /** 选中形象（custom:<id>，内建与自定义同通道） */
  onSelect: (id: string) => void;
  /** 宠物清单控制器（useCustomPets 实例） */
  controller: CustomPetsController;
}) {
  const { t } = useI18n();
  const builtins = controller.pets.filter((p) => p.builtin);
  const customs = controller.pets.filter((p) => !p.builtin);
  return (
    <div className="flex flex-col gap-2">
      {/* 内建形象组：内置智谱娘（无删除按钮——Rust 侧 delete 命令同样
          拒绝内置 id，双保险） */}
      <span className="text-[9px] text-slate-500">{t("theme.petGroupBuiltin")}</span>
      {builtins.length > 0 ? (
        <div className="grid grid-cols-2 gap-1.5">
          {builtins.map((pet) => {
            const styleValue = customStyleValue(pet.id);
            return (
              <button
                key={pet.id}
                onClick={() => onSelect(styleValue)}
                disabled={disabled}
                className={`flex flex-col items-center gap-1 rounded-md border py-2 transition-colors disabled:opacity-40 ${
                  value === styleValue
                    ? "border-sky-500 bg-sky-500/10"
                    : "border-slate-900/10 bg-slate-900/4 hover:border-slate-900/25"
                }`}
              >
                <PetThumb entry={pet} />
              </button>
            );
          })}
        </div>
      ) : (
        /* 清单未加载/释放失败的占位（Rust 启动时释放内置形象，正常
            稍后即出现） */
        <p className="text-[9px] text-slate-500 leading-relaxed">
          {t("theme.petBuiltinLoading")}
        </p>
      )}

      {/* 自定义形象组（Petdex 导入） */}
      <span className="text-[9px] text-slate-500 pt-1">
        {t("theme.petGroupCustom")}
      </span>
      {customs.length > 0 ? (
        <div className="grid grid-cols-2 gap-1.5">
          {customs.map((pet) => {
            const styleValue = customStyleValue(pet.id);
            const active = value === styleValue;
            return (
              <div
                key={pet.id}
                className={`relative flex flex-col items-center gap-1 rounded-md border py-2 transition-colors ${
                  active
                    ? "border-sky-500 bg-sky-500/10"
                    : "border-slate-900/10 bg-slate-900/4"
                }`}
              >
                <button
                  onClick={() => onSelect(styleValue)}
                  disabled={disabled}
                  className="flex flex-col items-center gap-1 disabled:opacity-40 w-full"
                >
                  <PetThumb entry={pet} />
                </button>
                <button
                  onClick={() => void controller.removeById(pet.id, pet.name)}
                  disabled={disabled}
                  title={t("theme.petDelete")}
                  aria-label={t("theme.petDelete")}
                  className="absolute top-0.5 right-0.5 w-4 h-4 rounded text-[9px] leading-none text-slate-500 hover:text-red-600 hover:bg-red-500/10 disabled:opacity-40"
                >
                  ×
                </button>
              </div>
            );
          })}
        </div>
      ) : (
        <p className="text-[9px] text-slate-500 leading-relaxed">
          {t("theme.petCustomEmpty")}
        </p>
      )}

      {/* 导入区：拖放目标提示（文件拖到面板窗口即导入，见宿主面板的
          onDragDropEvent 路由；皮肤页已安装时 png/webp 走壁纸导入，
          仅未安装时路由宠物导入，见 PET_IMPORT_IMAGE_RE） */}
      <div className="rounded-md border border-dashed border-slate-900/15 px-2 py-1.5 text-center">
        <p className="text-[9px] text-slate-500 leading-relaxed">
          {controller.importing
            ? t("theme.petImporting")
            : t("theme.petImportHintSkin")}
        </p>
        {controller.error && (
          <p className="text-[9px] text-red-600 leading-relaxed break-words">
            {controller.error}
          </p>
        )}
      </div>
    </div>
  );
}

/**
 * 桌面宠物形象清单（共享模块）：皮肤注入版设置卡（ThemePanel）与
 * 独立悬浮窗宠物设置卡（SettingsPanel）共用同一份形象选择数据，
 * 与注入脚本/宠物窗口加载的 pet-core.js 内嵌形象库（PET_STYLES）的键
 * 保持一致（Rust 侧默认值同源）。preview 为面板预览用的精简帧数据副本
 * （每形象取 idle 首帧，16×16 字符网格："." 透明、"1".."8" 调色板
 * 下标），仅服务形象选择器的静态展示，动画帧以 pet-core.js 为准。
 *
 * 第三阶段新增自定义宠物（Petdex 格式导入）：useCustomPets 提供清单/
 * 拖放导入/删除的共享状态与操作，PetStyleSection 渲染「内建 + 自定义」
 * 分组选择器（自定义卡用 Rust 生成的 idle 首帧缩略图，不加载全量图集），
 * 两处设置卡直接复用。
 */
import { useCallback, useEffect, useRef, useState } from "react";
import { useI18n, type MessageKey } from "./i18n";
import { deleteCustomPet, importPet, listCustomPets } from "./api";
import { PET_SIZE_LEVEL_PCT, type CustomPetEntry } from "./types";
import { PillGroup, PillButton } from "./layout";

export interface PetStyleOption {
  id: string;
  nameKey: MessageKey;
  palette: ReadonlyArray<string>; // 下标 1..8 的颜色（不含透明占位）
  preview: ReadonlyArray<string>; // idle 首帧 16 行
}

export const PET_STYLE_OPTIONS: ReadonlyArray<PetStyleOption> = [
  {
    id: "cat",
    nameKey: "theme.petStyleCat",
    palette: [
      "#3f3a39",
      "#f2b258",
      "#ffe0a3",
      "#e8837a",
      "#2f9e63",
      "#ffffff",
      "#7aa7f0",
      "#c97f3b",
    ],
    preview: [
      "................",
      "...11......11...",
      "..1221....1221..",
      "..1421....1241..",
      "..111111111111..",
      "..122222222221..",
      "..125222222521..",
      "..122222222221..",
      "..124422224421..",
      "..122226622221..",
      "..112222222211..",
      "..132222222231..",
      ".13222222222231.",
      ".13222222222231.",
      "..111111111111..",
      "................",
    ],
  },
  {
    id: "bot",
    nameKey: "theme.petStyleBot",
    palette: [
      "#2f3542",
      "#e9eef5",
      "#1c2430",
      "#57d4e8",
      "#e8574d",
      "#8b98ab",
      "#ffffff",
      "#6ea8f5",
    ],
    preview: [
      ".......5........",
      ".......1........",
      "..111111111111..",
      "..133333333331..",
      "..134433344331..",
      "..133333333331..",
      "..111111111111..",
      "...1111111111...",
      "..122222222221..",
      "..122277222221..",
      "..122222222221..",
      "..162222222261..",
      "..111111111111..",
      "...11....11.....",
      "...11....11.....",
      "................",
    ],
  },
];

/** 形象选择器的像素预览（16×16 网格按调色板着色） */
export function PetPreview({ option }: { option: PetStyleOption }) {
  return (
    <div
      className="grid"
      style={{
        gridTemplateColumns: "repeat(16, 1fr)",
        width: 40,
        height: 40,
      }}
      aria-hidden="true"
    >
      {option.preview.flatMap((row, y) =>
        Array.from(row).map((ch, x) => {
          const idx = ch === "." ? 0 : parseInt(ch, 10);
          const color = idx > 0 ? option.palette[idx - 1] : undefined;
          return (
            <div key={`${y}-${x}`} style={color ? { backgroundColor: color } : undefined} />
          );
        })
      )}
    </div>
  );
}

// ===== 自定义宠物（Petdex 导入）=====

/** 宠物导入文件（拖放路由用）：zip 包 / pet.json 元信息 */
export const PET_IMPORT_FILE_RE = /\.(zip|json)$/i;
/** 宠物导入图集文件（拖放路由用）：仅设置页路由到宠物导入
 *  （皮肤页的 png/webp 投放保留壁纸导入语义） */
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
 * 宠物尺寸档位选择器（屏高比例档位 1~5）：两处设置卡共用（设置页独立
 * 悬浮窗 + 皮肤页注入版），替代旧 48~128px 滑杆——按屏幕高度百分比定
 * 档（5.5%~15%），高分屏/低分屏观感一致，px 换算在 Rust 侧完成。分段
 * 控件与设置页字号/窗口大小的 PillGroup 风格一致；离散点击即时保存
 * （无滑杆拖动的防抖需求）。
 */
export function PetSizeLevelPicker({
  labelKey,
  value,
  disabled,
  onSelect,
}: {
  /** 标签词条（设置页 "settings.petSize" / 皮肤页 "theme.paramPetSize"） */
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

/** 自定义宠物的 style 值（pet_style / PetConfig.style 持久化形态） */
export const customStyleValue = (id: string) => `custom:${id}`;

/** useCustomPets 的控制器形态（PetStyleSection 与拖放路由共用） */
export interface CustomPetsController {
  /** 自定义宠物清单（按 id 排序） */
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
  /** 删除自定义宠物（确认浮层在内部弹出） */
  removeById: (id: string, name: string) => Promise<void>;
}

/**
 * 自定义宠物的共享状态与操作（两处设置卡各持一份实例，导入/删除后
 * 各自刷新清单并经 onChanged 回调让宿主面板重拉参数——删除正在使用
 * 的宠物时 Rust 侧会把两条管道的选中回退内建默认形象，面板需要重新
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
      /* 清单读取失败静默：选择器仍显示内建形象，不阻塞面板 */
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

/**
 * 宠物形象选择器（内建 + 自定义分组 + 导入区）。两处设置卡复用：
 * - 内建组：像素预览卡（既有形态）；
 * - 自定义组：Rust 生成的 idle 首帧缩略图卡（带删除按钮与格式角标）；
 * - 导入区：拖放目标提示（原生文件对话框在 Accessory 应用不可用，
 *   与壁纸导入同样走 Tauri 拖放事件，由宿主面板路由到
 *   controller.importFromPath）。
 */
export function PetStyleSection({
  value,
  disabled,
  onSelect,
  controller,
  skinPage = false,
}: {
  /** 当前选中形象 id（cat / bot / custom:<id>） */
  value: string;
  /** 操作禁用（皮肤页安装进行中等） */
  disabled?: boolean;
  /** 选中形象（内建 id 或 custom:<id>） */
  onSelect: (id: string) => void;
  /** 自定义宠物控制器（useCustomPets 实例） */
  controller: CustomPetsController;
  /** 皮肤页形态：png/webp 投放在皮肤页路由给壁纸导入，导入提示只列
   *  zip / pet.json（P2-2，与 ThemePanel 的拖放路由保持一致） */
  skinPage?: boolean;
}) {
  const { t } = useI18n();
  return (
    <div className="flex flex-col gap-2">
      {/* 内建形象组 */}
      <span className="text-[9px] text-slate-500">{t("theme.petGroupBuiltin")}</span>
      <div className="grid grid-cols-2 gap-1.5">
        {PET_STYLE_OPTIONS.map((opt) => (
          <button
            key={opt.id}
            onClick={() => onSelect(opt.id)}
            disabled={disabled}
            className={`flex flex-col items-center gap-1 rounded-md border py-2 transition-colors disabled:opacity-40 ${
              value === opt.id
                ? "border-sky-500 bg-sky-500/10"
                : "border-slate-900/10 bg-slate-900/4 hover:border-slate-900/25"
            }`}
          >
            <PetPreview option={opt} />
            <span className="text-[9px] text-slate-600">{t(opt.nameKey)}</span>
          </button>
        ))}
      </div>

      {/* 自定义形象组（Petdex 导入） */}
      <span className="text-[9px] text-slate-500 pt-1">
        {t("theme.petGroupCustom")}
      </span>
      {controller.pets.length > 0 ? (
        <div className="grid grid-cols-2 gap-1.5">
          {controller.pets.map((pet) => {
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
                  {pet.thumb ? (
                    <img
                      src={pet.thumb}
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
                    {pet.name}
                  </span>
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

      {/* 导入区：拖放目标提示（文件拖到面板窗口即导入，见两处设置卡的
          onDragDropEvent 路由；皮肤页 png/webp 走壁纸导入，提示按页面
          区分，P2-2） */}
      <div className="rounded-md border border-dashed border-slate-900/15 px-2 py-1.5 text-center">
        <p className="text-[9px] text-slate-500 leading-relaxed">
          {controller.importing
            ? t("theme.petImporting")
            : skinPage
              ? t("theme.petImportHintSkin")
              : t("theme.petImportHint")}
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

/**
 * 独立桌面宠物窗口宿主壳（宠物功能第二阶段）。
 *
 * 宠物核心（/pet-core.js 的 ZBarPet 工厂）无环境假设，本壳负责把
 * Tauri 环境接入核心——与皮肤注入版宿主壳（inject.rs PET_JS 尾段）：
 * - 参数来源：初始经 get_pet_config 命令读取；后续监听 zbar://pet-params
 *   事件（设置页改形象/大小时 Rust 侧推送）调 setParams；
 * - 数据来源：监听 zbar://pet-usage 事件（pet.rs 的独立轮询器每 2 秒
 *   推送的 usage-data.js 同构摘要）喂 feed；
 * - 自定义形象（第三阶段，V8 起为全部形象的唯一通道）：style 形如
 *   custom:<id>（默认内置智谱娘）时经 get_custom_pet_asset 命令读取
 *   资产（meta + 图集 dataUri）作为 customAsset 传给核心；资产读取
 *   失败（宠物被删除/元信息损坏）静默进入空态——核心不渲染（宠物暂
 *   隐，V8 起无内建回退）；参数事件每次抵达都重取资产（Rust 侧在导
 *   入替换后会重推参数事件，同 id 的资产对象变化触发核心重建画布，
 *   帧数据即时刷新）；
 * - 拖动：#pet-root 为 data-tauri-drag-region 拖动区；核心创建的 canvas
 *   是动态子元素，创建/重建后由本壳补挂拖动属性（窗口大小 = 宠物大小，
 *   几乎无空白区，画布本身必须可拖）。
 *
 * 防御式风格与注入版一致：核心缺失/容器缺失时静默退出，监听失败不
 * 阻塞已创建的宠物（保持沉睡观感）。
 */
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { DEFAULT_PET_STYLE, petSizePx } from "./types";
import type { CustomPetAsset } from "./types";

/** ZBarPet 工厂创建的宠物实例接口（pet-core.js 契约的 TS 描述） */
interface ZBarPetInstance {
  feed(data: unknown): void;
  /** 喂心跳（陈旧判定用；独立版以事件到达时刻为心跳） */
  heartbeat(ms: number): void;
  setParams(params: {
    style?: string;
    size?: number;
    customAsset?: CustomPetAsset | undefined;
  }): void;
  destroy(): void;
}

/** pet-core.js 暴露的全局工厂 */
interface ZBarPetFactory {
  create(
    container: HTMLElement,
    opts: {
      style?: string;
      size?: number;
      customAsset?: CustomPetAsset | undefined;
    }
  ): ZBarPetInstance | null;
}

declare global {
  interface Window {
    ZBarPet?: ZBarPetFactory;
  }
}

/** Rust 侧配置命令的返回形态（PetConfig，仅消费参数字段） */
interface PetConfigPayload {
  enabled: boolean;
  style: string;
  size: number;
}

/** 自定义形象前缀（与 pet-core.js / Rust pets 模块约定一致） */
const CUSTOM_STYLE_PREFIX = "custom:";

/** 给容器内全部画布补挂 Tauri 拖动属性（形象热切换会重建画布） */
const markDragRegion = (root: HTMLElement) => {
  root.querySelectorAll("canvas").forEach((c) =>
    c.setAttribute("data-tauri-drag-region", "")
  );
};

/**
 * 读取自定义形象渲染资产：style 非 custom:* 返回 undefined；命令失败
 * （宠物已删除/元信息损坏）同样返回 undefined，核心对 custom 样式缺
 * 资产进入空态（不渲染，V8 起无内建回退——资产就位后随参数事件热切换
 * 恢复）。
 */
const fetchCustomAsset = async (
  style: string
): Promise<CustomPetAsset | undefined> => {
  if (!style.startsWith(CUSTOM_STYLE_PREFIX)) return undefined;
  const id = style.slice(CUSTOM_STYLE_PREFIX.length);
  if (!id) return undefined;
  try {
    return await invoke<CustomPetAsset>("get_custom_pet_asset", { id });
  } catch {
    return undefined; /* 静默回退内建形象 */
  }
};

const main = async () => {
  const root = document.getElementById("pet-root");
  if (!root || !window.ZBarPet) return; // 核心未加载（异常环境）静默退出

  // 初始参数：窗口由 Rust 侧创建（尺寸/位置已按配置就位），此处只取
  // 形象与边长渲染宠物本体；读取失败按核心默认（内置智谱娘 / 档 3）
  // 渲染（DEFAULT_PET_STYLE 与 Rust 侧同源；V8 起合法值恒为 custom:*）。
  // 配置的 size 为档位（1~5），渲染前按本窗口所在屏幕逻辑高换算成 px
  // （window.screen.height 为 webview 逻辑像素，与 Rust 侧建窗换算同
  // 口径；取不到时 petSizePx 内部兜底 1080）。后续参数变化经
  // zbar://pet-params 事件接收，其 size 已由 Rust 侧换算为 px
  let style = DEFAULT_PET_STYLE;
  let size = 70; /* 档 3 @1080 兜底观感 */
  try {
    const cfg = await invoke<PetConfigPayload>("get_pet_config");
    style = cfg.style || style;
    if (cfg.size) size = petSizePx(cfg.size, window.screen?.height ?? 0);
  } catch {
    /* 静默：保持默认参数 */
  }
  const asset = await fetchCustomAsset(style);
  const pet = window.ZBarPet.create(root, { style, size, customAsset: asset });
  if (!pet) return; // 画布不可用（极端环境）静默退出
  markDragRegion(root);

  // 数据流：独立轮询器推送的 usage-data.js 同构摘要（v/ts/la/turns/runs）。
  // 事件到达本身即"数据源存活"的信号——每收到一次就喂一次心跳
  //（heartbeat），核心陈旧判定据此感知 ZBar/轮询器退出（事件停流 →
  // 心跳停滞 → 沉睡）；查询失败时轮询器静默跳过不推事件，天然降级。
  try {
    await listen("zbar://pet-usage", (e) => {
      pet.heartbeat(Date.now());
      pet.feed(e.payload);
    });
  } catch {
    /* 监听失败保持沉睡观感（无数据则宠物自然入睡） */
  }

  // 参数流：设置页改形象/大小时 Rust 侧推送（窗口尺寸由 Rust 侧同步调整）。
  // custom:* 形象每次都重取资产（导入替换后 Rust 会重推参数事件）
  try {
    await listen<{ style?: string; size?: number }>(
      "zbar://pet-params",
      (e) => {
        const nextStyle = e.payload?.style;
        void (async () => {
          const customAsset = await fetchCustomAsset(nextStyle ?? "");
          pet.setParams({ ...e.payload, customAsset });
          markDragRegion(root); // 形象热切换重建画布后补挂拖动属性
        })();
      }
    );
  } catch {
    /* 静默：参数保持初始值 */
  }
};

void main();

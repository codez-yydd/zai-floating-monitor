/**
 * localStorage 缓存工具：用于面板首屏秒开。
 *
 * 背景：面板窗口以 visible:false 启动，首次打开时 WebView 冷启动会白屏。
 * 配合"屏幕外预热"（WebView 提前渲染），把上次查询结果缓存到本地，
 * 打开瞬间先用缓存数据渲染，后台再刷新实时数据覆盖。
 *
 * 所有操作都 try-catch：配额超限、序列化失败、隐私模式等情况下静默降级，
 * 绝不影响正常的数据加载流程（缓存只是锦上添花）。
 */

/** 读取缓存；不存在或解析失败返回 null。 */
export function loadCache<T>(key: string): T | null {
  try {
    const raw = localStorage.getItem(key);
    return raw ? (JSON.parse(raw) as T) : null;
  } catch {
    return null;
  }
}

/** 写入缓存；失败静默（不抛错，避免影响主流程）。 */
export function saveCache<T>(key: string, data: T): void {
  try {
    localStorage.setItem(key, JSON.stringify(data));
  } catch {
    // 忽略：QuotaExceededError、隐私模式、序列化失败等
  }
}

/**
 * localStorage 持久化缓存：用于面板冷启动首屏秒开。
 *
 * DataProvider 把数据常驻在 React state（内存）里，热重显（切页面再切回）
 * 能瞬时恢复；但进程重启后内存清空，冷启动仍需等首次请求。这里把上次结果
 * 同步落盘，冷启动时先用缓存渲染、后台请求刷新后覆盖 —— 与内存缓存互补。
 *
 * 所有操作 try-catch：配额超限、序列化失败、隐私模式等情况下静默降级，
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

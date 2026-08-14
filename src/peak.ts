// 高峰期折算：前端复刻 Rust peak.rs 的 multiplier_at / credits_for_call / zcode_factor。
// 用于把远端逐条明细折算成消耗（服务端无 peak 配置，折算在客户端做）。
// 系数表与 src-tauri/src/peak.rs 保持一致，改动需同步。

import type { PeakConfig } from "./types";

/** V3 积分系数表（与 Rust credit_coef 一致）。key = model_id（小写）。 */
const CREDIT_COEF: Record<string, { input: number; cache: number; output: number }> = {
  "glm-5.2": { input: 6.9, cache: 1.7, output: 24.0 },
  "glm-5-turbo": { input: 5.7, cache: 1.5, output: 21.0 },
  "glm-4.7": { input: 4.6, cache: 1.2, output: 16.0 },
  "glm-4.6v": { input: 1.2, cache: 0.3, output: 2.7 },
};

/** ZCode 优惠系数（与 Rust ZCODE_DISCOUNT 一致） */
const ZCODE_DISCOUNT = 0.67;

/** "HH:MM" → 当天分钟数 (0-1439)，非法返回 null。
 * 与 Rust parse_hhmm 严格对齐：仅接受 trim 后的纯数字段（Rust u32 parse
 * 还接受 '+' 前缀，如 "+5:30" 按分钟 5*60+30 处理），拒绝空段
 * （Number("")=0）、科学计数（Number("1e1")=10）、多段冒号（"1:2:3"）等
 * JS 宽松解析值，避免两端折算结果分叉。 */
function parseHhmm(s: string): number | null {
  const parts = s.split(":");
  if (parts.length !== 2) return null; // Rust split_once 后余段会 parse 失败
  const re = /^\+?\d+$/; // Rust u32 parse 语义：可选 '+' 前缀 + 纯数字（含任意位数）
  const [h, m] = parts;
  if (!re.test(h.trim()) || !re.test(m.trim())) return null;
  const hi = Number(h.trim());
  const mi = Number(m.trim());
  if (hi > 23 || mi > 59) return null;
  return hi * 60 + mi;
}

/** ms → (weekday_bit, 当天分钟数)。
 * weekday_bit: 0=周日 ... 6=周六（与 Rust peak.rs 一致） */
function msToLocal(ms: number): [number, number] | null {
  const d = new Date(ms);
  if (isNaN(d.getTime())) return null;
  // JS getDay(): 0=周日 ... 6=周六，与 Rust weekday_bit 编码一致
  const weekdayBit = d.getDay();
  const nowMin = d.getHours() * 60 + d.getMinutes();
  return [weekdayBit, nowMin];
}

/** 判断 ms 落在哪个时段，返回对应倍率（未启用/未匹配 → 1.0）。
 * 支持跨午夜时段（end < start，如 22:00-02:00 = [22:00,24:00) ∪ [00:00,02:00)）。
 * 与 Rust multiplier_at 一致。 */
export function multiplierAt(ms: number, cfg: PeakConfig): number {
  if (!cfg.enabled || cfg.segments.length === 0) return 1.0;
  const local = msToLocal(ms);
  if (!local) return 1.0;
  const [weekdayBit, nowMin] = local;
  for (const seg of cfg.segments) {
    if (((seg.weekday_mask >> weekdayBit) & 1) !== 1) continue;
    const start = parseHhmm(seg.start);
    const end = parseHhmm(seg.end);
    if (start === null || end === null) continue;
    // 跨午夜区间（end < start）匹配 [start,24:00) ∪ [00:00,end)；
    // end === start 视为空区间不匹配（否则会被误放大成全天命中）
    const hit =
      end > start
        ? nowMin >= start && nowMin < end
        : end < start
          ? nowMin >= start || nowMin < end
          : false;
    if (hit) {
      return seg.multiplier;
    }
  }
  return 1.0;
}

/** ZCode 优惠系数（启用 → 0.67，否则 1.0） */
export function zcodeFactor(cfg: PeakConfig): number {
  return cfg.zcode_discount ? ZCODE_DISCOUNT : 1.0;
}

/** V3 单条调用的积分消耗（含时段倍率，不含 ZCode 优惠）。
 * 模型无系数返回 null。与 Rust credits_for_call 一致。 */
export function creditsForCall(
  modelId: string,
  inputTokens: number,
  cacheReadTokens: number,
  outputTokens: number,
  multiplier: number
): number | null {
  const c = CREDIT_COEF[modelId.toLowerCase()];
  if (!c) return null;
  const base =
    (inputTokens * c.input +
      cacheReadTokens * c.cache +
      outputTokens * c.output) /
    10_000;
  return base * multiplier;
}

/** 把一组周期内的远端明细折算成消耗，与本地 db::query_period_consumed 口径一致。
 * - V2：Σ total_tokens × 时段倍率
 * - V3：Σ 积分公式 × 时段倍率（无系数模型按 0 计）
 * - 都 × ZCode 优惠系数
 * 返回每个周期的 {consumed, requests}，顺序与 periods 一致。 */
export function detailToConsumed(
  detail: { rows: { started_at: number; model_id: string; input_tokens: number; output_tokens: number; cache_read_tokens: number; total_tokens: number }[] }[],
  cfg: PeakConfig
): { consumed: number; requests: number }[] {
  const zcode = zcodeFactor(cfg);
  const plan = cfg.plan_type;
  return detail.map((bucket) => {
    let consumed = 0;
    let requests = 0;
    for (const r of bucket.rows) {
      const mult = multiplierAt(r.started_at, cfg);
      let callConsumed = 0;
      if (plan === "v3") {
        callConsumed =
          creditsForCall(
            r.model_id,
            r.input_tokens,
            r.cache_read_tokens,
            r.output_tokens,
            mult
          ) ?? 0;
      } else if (plan === "v2") {
        callConsumed = r.total_tokens * mult;
      }
      consumed += callConsumed * zcode;
      requests += 1;
    }
    return { consumed, requests };
  });
}

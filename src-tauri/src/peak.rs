//! 高峰期配置 + 额度折算（V2 按请求倍率 / V3 按积分公式）。
//!
//! 设计要点：
//! - 配置存储：~/.zbar/peak.json
//! - 订阅类型 plan_type：用户必须选择 V2 或 V3（不预选），决定折算口径
//! - ZCode 150% 提额优惠 zcode_discount：独立开关，对 V2/V3 都生效（全周期 ×0.67）
//! - 时段倍率 segments：高峰/非高峰的倍率，V2 和 V3 默认值不同
//!
//! 折算口径：
//! - V2：消耗 = total_tokens × 时段倍率（高峰3x/非高峰1x）
//! - V3：消耗 = 积分 = (input×in + cache×cache + output×out)/10000 × 时段倍率(高峰1.0/非高峰0.5)
//! - 两者最后都可 × ZCode 0.67（若启用优惠）

use chrono::{Datelike, TimeZone, Timelike};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

use crate::pricing::config_dir;

// ===== weekday 位掩码常量（用位移定义，杜绝手算二进制出错）=====
// 约定：bit0=周日, bit1=周一, ..., bit6=周六（与 multiplier_at 的 weekday_bit 一一对应）
pub const WD_SUN: u32 = 1 << 0;
pub const WD_MON: u32 = 1 << 1;
pub const WD_TUE: u32 = 1 << 2;
pub const WD_WED: u32 = 1 << 3;
pub const WD_THU: u32 = 1 << 4;
pub const WD_FRI: u32 = 1 << 5;
pub const WD_SAT: u32 = 1 << 6;
/// 工作日：周一~周五
pub const MASK_WEEKDAY: u32 = WD_MON | WD_TUE | WD_WED | WD_THU | WD_FRI; // = 62
/// 周末：周六+周日
pub const MASK_WEEKEND: u32 = WD_SUN | WD_SAT; // = 65

/// ZCode 150% 提额优惠系数（全周期按此折算额度消耗）
pub const ZCODE_DISCOUNT: f64 = 0.67;

/// 订阅类型：决定折算口径。None 表示用户尚未选择（不折算）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum PlanType {
    V2,
    V3,
}

/// 单个高峰时段配置。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakSegment {
    /// 起始时间 "HH:MM"
    #[serde(default = "default_start")]
    pub start: String,
    /// 结束时间 "HH:MM"
    #[serde(default = "default_end")]
    pub end: String,
    /// 倍率：V2 高峰 3.0/非高峰 1.0；V3 高峰 1.0/非高峰 0.5
    #[serde(default = "default_peak_mult")]
    pub multiplier: f64,
    /// 周几位掩码：用 MASK_WEEKDAY / MASK_WEEKEND 常量，勿手算二进制
    #[serde(default = "default_weekday")]
    pub weekday_mask: u32,
}

fn default_start() -> String {
    "14:00".to_string()
}
fn default_end() -> String {
    "18:00".to_string()
}
fn default_peak_mult() -> f64 {
    1.0
}
fn default_weekday() -> u32 {
    MASK_WEEKDAY
}

/// V2 默认时段（不含 ZCode 优惠）：
/// - 工作日 14:00-18:00 高峰 3.0x
/// - 工作日其余时段 + 周末全天 非高峰 1.0x
fn v2_segments() -> Vec<PeakSegment> {
    vec![
        PeakSegment {
            start: "14:00".into(),
            end: "18:00".into(),
            multiplier: 3.0,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "00:00".into(),
            end: "14:00".into(),
            multiplier: 1.0,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "18:00".into(),
            end: "23:59".into(),
            multiplier: 1.0,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "00:00".into(),
            end: "23:59".into(),
            multiplier: 1.0,
            weekday_mask: MASK_WEEKEND,
        },
    ]
}

/// V3 默认时段（不含 ZCode 优惠）：
/// - 工作日 14:00-18:00 高峰 1.0x
/// - 工作日其余时段 + 周末全天 非高峰 0.5x
fn v3_segments() -> Vec<PeakSegment> {
    vec![
        PeakSegment {
            start: "14:00".into(),
            end: "18:00".into(),
            multiplier: 1.0,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "00:00".into(),
            end: "14:00".into(),
            multiplier: 0.5,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "18:00".into(),
            end: "23:59".into(),
            multiplier: 0.5,
            weekday_mask: MASK_WEEKDAY,
        },
        PeakSegment {
            start: "00:00".into(),
            end: "23:59".into(),
            multiplier: 0.5,
            weekday_mask: MASK_WEEKEND,
        },
    ]
}

/// 高峰期配置（订阅类型 + ZCode优惠 + 时段）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeakConfig {
    /// 订阅类型：None 表示用户尚未选择（首次运行不预选）
    #[serde(default)]
    pub plan_type: Option<PlanType>,
    /// ZCode 150% 提额优惠开关（独立于订阅类型）
    #[serde(default)]
    pub zcode_discount: bool,
    /// 是否启用折算
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    /// 时段列表
    #[serde(default)]
    pub segments: Vec<PeakSegment>,
}

fn default_enabled() -> bool {
    true
}

impl Default for PeakConfig {
    fn default() -> Self {
        // 首次运行：不预选订阅类型，空时段，不折算
        Self {
            plan_type: None,
            zcode_discount: false,
            enabled: false,
            segments: Vec::new(),
        }
    }
}

impl PeakConfig {
    /// 按订阅类型重置为官方默认时段（保留 zcode_discount 设置）。
    pub fn reset_for_plan(&mut self, plan: PlanType) {
        self.plan_type = Some(plan);
        self.segments = match plan {
            PlanType::V2 => v2_segments(),
            PlanType::V3 => v3_segments(),
        };
        self.enabled = true;
    }
}

/// ~/.zbar/peak.json
pub fn config_path() -> Result<PathBuf, String> {
    Ok(config_dir()?.join("peak.json"))
}

/// 读取高峰期配置；文件不存在返回默认（未选择订阅类型）。
pub fn load_peak() -> Result<PeakConfig, String> {
    let path = config_path()?;
    if !path.exists() {
        return Ok(PeakConfig::default());
    }
    let data = std::fs::read_to_string(&path)
        .map_err(|e| format!("读取高峰期配置失败: {e}"))?;
    serde_json::from_str::<PeakConfig>(&data)
        .map_err(|e| format!("解析高峰期配置失败: {e}"))
}

/// 写入高峰期配置。
pub fn save_peak(cfg: &PeakConfig) -> Result<(), String> {
    let dir = config_dir()?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("创建配置目录失败: {e}"))?;
    let path = config_path()?;
    let data = serde_json::to_string_pretty(cfg)
        .map_err(|e| format!("序列化高峰期配置失败: {e}"))?;
    std::fs::write(&path, data).map_err(|e| format!("写入高峰期配置失败: {e}"))
}

/// "HH:MM" → 当天的分钟数 (0-1439)。解析失败返回 None。
fn parse_hhmm(s: &str) -> Option<u32> {
    let (h, m) = s.split_once(':')?;
    let h: u32 = h.trim().parse().ok()?;
    let m: u32 = m.trim().parse().ok()?;
    if h > 23 || m > 59 {
        return None;
    }
    Some(h * 60 + m)
}

/// ms → (weekday_bit, 当天分钟数)。失败返回 None。
fn ms_to_local(ms: i64) -> Option<(u32, u32)> {
    let dt = chrono::Local.timestamp_millis_opt(ms).single()?;
    let weekday_bit: u32 = match dt.weekday() {
        chrono::Weekday::Sun => 0,
        chrono::Weekday::Mon => 1,
        chrono::Weekday::Tue => 2,
        chrono::Weekday::Wed => 3,
        chrono::Weekday::Thu => 4,
        chrono::Weekday::Fri => 5,
        chrono::Weekday::Sat => 6,
    };
    let now_min = dt.hour() * 60 + dt.minute();
    Some((weekday_bit, now_min))
}

/// 判断毫秒时间戳落在哪个时段，返回对应倍率。
/// - 未启用/无时段/未匹配 → 返回 1.0（基础倍率）
/// - 取第一个匹配的 segment（用户应避免配置重叠时段）
/// - 支持跨午夜时段（end < start，如 22:00-02:00 = [22:00,24:00) ∪ [00:00,02:00)）
pub fn multiplier_at(ms: i64, cfg: &PeakConfig) -> f64 {
    if !cfg.enabled || cfg.segments.is_empty() {
        return 1.0;
    }
    let Some((weekday_bit, now_min)) = ms_to_local(ms) else {
        return 1.0;
    };
    for seg in &cfg.segments {
        if (seg.weekday_mask >> weekday_bit) & 1 != 1 {
            continue;
        }
        let Some(start) = parse_hhmm(&seg.start) else {
            continue;
        };
        let Some(end) = parse_hhmm(&seg.end) else {
            continue;
        };
        // 跨午夜区间（end < start）匹配 [start,24:00) ∪ [00:00,end)；
        // end == start 视为空区间不匹配（否则会被误放大成全天命中）
        let hit = if end > start {
            now_min >= start && now_min < end
        } else if end < start {
            now_min >= start || now_min < end
        } else {
            false
        };
        if hit {
            return seg.multiplier;
        }
    }
    1.0
}

/// ZCode 优惠系数：启用返回 0.67，否则 1.0。
pub fn zcode_factor(cfg: &PeakConfig) -> f64 {
    if cfg.zcode_discount {
        ZCODE_DISCOUNT
    } else {
        1.0
    }
}

// ===== V3 积分系数表（来自官方文档）=====

/// 单个模型的积分抵扣系数（V3 用）。
/// 积分 = (input×in + cache_read×cache + output×out) / 10000
#[derive(Debug, Clone, Copy)]
pub struct CreditCoef {
    pub input: f64,
    pub cache: f64,
    pub output: f64,
}

/// V3 积分系数表。key = model_id（小写匹配）。
/// 来源：智谱 GLM Coding Plan 用量说明文档。
pub fn credit_coef(model_id: &str) -> Option<CreditCoef> {
    match model_id.to_lowercase().as_str() {
        "glm-5.2" => Some(CreditCoef {
            input: 6.9,
            cache: 1.7,
            output: 24.0,
        }),
        "glm-5-turbo" => Some(CreditCoef {
            input: 5.7,
            cache: 1.5,
            output: 21.0,
        }),
        "glm-4.7" => Some(CreditCoef {
            input: 4.6,
            cache: 1.2,
            output: 16.0,
        }),
        "glm-4.6v" => Some(CreditCoef {
            input: 1.2,
            cache: 0.3,
            output: 2.7,
        }),
        // MCP 工具按调用次数 × output 系数（此处不处理 MCP，由调用方单独算）
        _ => None,
    }
}

/// V3 单条调用的积分消耗（不含 ZCode 优惠，含时段倍率）。
/// 返回 None 表示该模型无系数（无法折算）。
pub fn credits_for_call(
    model_id: &str,
    input_tokens: i64,
    cache_read_tokens: i64,
    output_tokens: i64,
    multiplier: f64,
) -> Option<f64> {
    let c = credit_coef(model_id)?;
    let base = (input_tokens as f64 * c.input
        + cache_read_tokens as f64 * c.cache
        + output_tokens as f64 * c.output)
        / 10_000.0;
    Some(base * multiplier)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 全周掩码：时段判断用例不关心星期几，避免对具体日期敏感
    const ALL_WEEK: u32 = MASK_WEEKDAY | MASK_WEEKEND;

    fn seg(start: &str, end: &str, multiplier: f64) -> PeakSegment {
        PeakSegment {
            start: start.into(),
            end: end.into(),
            multiplier,
            weekday_mask: ALL_WEEK,
        }
    }

    fn cfg_with(segments: Vec<PeakSegment>) -> PeakConfig {
        PeakConfig {
            plan_type: Some(PlanType::V2),
            zcode_discount: false,
            enabled: true,
            segments,
        }
    }

    /// 本地时间 → 毫秒时间戳（选 8 月中旬，避开各地夏令时切换的缺失时刻）
    fn local_ms(y: i32, mo: u32, d: u32, h: u32, mi: u32) -> i64 {
        chrono::Local
            .with_ymd_and_hms(y, mo, d, h, mi, 0)
            .single()
            .expect("测试时间应有效")
            .timestamp_millis()
    }

    /// 跨午夜区间（22:00-02:00）：23 点、次日 1 点命中，正午不命中
    #[test]
    fn multiplier_at_cross_midnight_hits() {
        let cfg = cfg_with(vec![seg("22:00", "02:00", 3.0)]);
        // 2026-08-12 周三 / 13 日周四，跨到次日验证 [00:00,end) 半段
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 23, 0), &cfg), 3.0);
        assert_eq!(multiplier_at(local_ms(2026, 8, 13, 1, 0), &cfg), 3.0);
        assert_eq!(multiplier_at(local_ms(2026, 8, 13, 12, 0), &cfg), 1.0);
        // 边界：起点 22:00 命中（含），终点 02:00 不命中（排他）
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 22, 0), &cfg), 3.0);
        assert_eq!(multiplier_at(local_ms(2026, 8, 13, 2, 0), &cfg), 1.0);
    }

    /// 正常区间（14:00-18:00）行为不回归：区间内命中，起点前/终点整点不命中
    #[test]
    fn multiplier_at_normal_range() {
        let cfg = cfg_with(vec![seg("14:00", "18:00", 3.0)]);
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 15, 0), &cfg), 3.0);
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 13, 59), &cfg), 1.0);
        // end 排他：18:00 整点已不在区间内
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 18, 0), &cfg), 1.0);
    }

    /// start == end 视为空区间，任何时刻都不命中（避免被跨夜逻辑放大成全天命中）
    #[test]
    fn multiplier_at_empty_range_never_hits() {
        let cfg = cfg_with(vec![seg("14:00", "14:00", 3.0)]);
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 14, 0), &cfg), 1.0);
        assert_eq!(multiplier_at(local_ms(2026, 8, 12, 9, 0), &cfg), 1.0);
    }
}
